#[macro_use]
extern crate log;
extern crate env_logger;
#[macro_use]
extern crate serde_derive; 
extern crate serde;
extern crate serde_json;
extern crate hyper;
extern crate futures;
extern crate tokio_core;

mod jresponse;

use futures::future::{self,Future,Either};
use futures::{
    Canceled,
    sync as fsync,
    Stream,
};

pub use hyper::{
    Chunk,Method,Headers,Uri,HttpVersion,StatusCode,
    server::{Http, Request, Response, Service, NewService, RemoteAddr, HasRemoteAddr},
    header::{ContentLength,ContentType,CacheControl,CacheDirective},
};

use tokio_core::reactor::{Core, Handle, Remote};

use std::{
    sync::Arc,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    collections::BTreeMap,
};
pub use jresponse::JsonResponse;

enum ReplySenderVariant {
    Oneshot(Option<fsync::oneshot::Sender<JsonResponse>>),
    Stream(fsync::mpsc::Sender<Result<Chunk,hyper::Error>>),
}

pub struct ReplySender {
    http_debug: bool,
    sender: ReplySenderVariant,
}
impl ReplySender {
    pub fn oneshot() -> (ReplySender,fsync::oneshot::Receiver<JsonResponse>) {
        let (tx_body, rx_body) = fsync::oneshot::channel();
        (ReplySender {
            http_debug: false,
            sender: ReplySenderVariant::Oneshot(Some(tx_body)),
        },rx_body)
    }
    pub fn send(&mut self, r: JsonResponse) -> Result<(),FrontendError> {
        match &mut self.sender {
            ReplySenderVariant::Oneshot(sender) => match sender.take() {
                Some(sender) => sender.send(r).map_err(|_| FrontendError::BackendSend),
                None => Err(FrontendError::NoSender),
            },
            ReplySenderVariant::Stream(_sender) => Err(FrontendError::NotImplemented),
        }
    }
}
pub enum DispatchResult<R> {
    Ok(R),
    ChunkedOk(R),
    Err(JsonResponse),
}

pub struct ReactorRemoteControl {
    _destructor: fsync::oneshot::Sender<()>,
    handle: Remote,
}
impl ReactorRemoteControl {
    pub fn ref_remote(&self) -> &Remote {
        &self.handle
    }
}
pub struct ReactorControl {
    _destructor: fsync::oneshot::Sender<()>,
    handle: Handle,
}
impl ReactorControl {
    pub fn remote(&self) -> Remote {
        self.handle.remote().clone()
    }
    pub fn handle(&self) -> Handle {
        self.handle.clone()
    }
    pub fn ref_handle(&self) -> &Handle {
        &self.handle
    }
    pub fn into_remote(self) -> ReactorRemoteControl {
        ReactorRemoteControl {
            _destructor: self._destructor,
            handle: self.handle.remote().clone(),
        }
    }
}

pub enum InitDestructor {
    Init,
    Ignore,
}

#[derive(Clone,Copy)]
pub struct ClientInfo(pub SocketAddr);

pub trait RequestDispatcher: Clone + Sized + 'static {
    type Request;    
    fn dispatch(&self, role: &str, http: &HttpVersion, method: &Method, uri: &Uri, headers: &Headers, body: &[u8]) -> DispatchResult<Self::Request>;
    fn process(&self, req: Self::Request, sender: ReplySender, info: ClientInfo);

    fn reactor_control(&mut self, _ctrl: ReactorControl) -> InitDestructor {
        InitDestructor::Ignore
    }
    fn connection_error_reporter(&self, _error: hyper::Error) {}
    fn http_debug(&self) -> bool { false }
}

#[derive(Debug)]
pub enum FrontendError {
    UnexpectedTermination,
    NoSender,
    NotImplemented,
    BackendSend,
    Json(serde_json::Error),
    Hyper(hyper::Error),
    Address(std::net::AddrParseError),
    Reactor(std::io::Error),
}

pub struct FrontendBuilder {
    addresses: BTreeMap<String,SocketAddr>,
}
impl FrontendBuilder {
    pub fn new() -> FrontendBuilder {
        FrontendBuilder {
            addresses: BTreeMap::new(),
        }
    }
    pub fn add_role(mut self, alias: &str, addr: &str) -> Result<FrontendBuilder,FrontendError> {
        let addr = addr.parse().map_err(FrontendError::Address)?;
        self.addresses.entry(alias.to_string()).or_insert(addr);
        Ok(self)
    }
    
    pub fn run<D: RequestDispatcher>(self, mut dispatcher: D) -> Result<(),FrontendError> {
        let mut core = Core::new().map_err(FrontendError::Reactor)?;
        let handle = core.handle();

        let (tx,destructor) = fsync::oneshot::channel::<()>();
        let init_destructor = dispatcher.reactor_control(ReactorControl{
            _destructor: tx,
            handle: core.handle(),
        });
        
        let mut vs = Vec::new();
        let shared_d = Arc::new(dispatcher);
        for (alias,addr) in self.addresses {
            info!("starting '{}' server: {}",alias,addr);
            vs.push(Http::new().serve_addr_handle(&addr,&handle,Session::new(alias,handle.clone(),shared_d.clone())).map_err(FrontendError::Hyper)?);
        };
        let serve_future = match vs.len() {
            0 => {
                error!("No addresses to process");
                return Ok(());
            },
            1 => {
                let dsp = shared_d.clone();
                Either::A(vs.pop().unwrap().for_each(move |conn| {
                    let dsp = dsp.clone();
                    info!("Incomming connection: {}",conn.remote());
                    handle.spawn(conn.map_err(move |e| dsp.connection_error_reporter(e)));
                    Ok(())
                }))
            },
            2 => {
                let dsp = shared_d.clone();
                Either::B(vs.pop().unwrap().select(vs.pop().unwrap()).for_each(move |conn| {
                    let dsp = dsp.clone();
                    info!("Incomming connection: {}",conn.remote());
                    handle.spawn(conn.map_err(move |e| dsp.connection_error_reporter(e)));
                    Ok(())
                }))
            },
            _ => {
                error!("too many addresses to process");
                return Ok(());
            },
        }.map_err(FrontendError::Hyper);
        
        let final_future = match init_destructor {
            InitDestructor::Init => Either::A({
                serve_future.select2({
                    destructor.or_else(|Canceled| future::ok(()))
                })
                    .map_err(|var_e| match var_e {
                        Either::A((e,..)) |
                        Either::B((e,..)) => e,
                    })
                    .and_then(|var_r| match var_r {
                        Either::A(((),..)) => future::err(FrontendError::UnexpectedTermination),
                        Either::B(((),..)) => future::ok(()),
                    })
            }),
            InitDestructor::Ignore => Either::B(serve_future.and_then(|()| future::err(FrontendError::UnexpectedTermination))),
        };
        core.run(final_future) 
    }
}

#[derive(Debug,Clone)]
struct Session<D>
    where D: RequestDispatcher
{
    role: String,
    handle: Handle,
    dispatcher: Arc<D>,
    remote_addr: SocketAddr, 
}
impl<D> Session<D>
    where D: RequestDispatcher
{
    fn new<S: ToString>(role: S, handle: Handle, dispatcher: Arc<D>) -> Session<D> {
        Session {
            role: role.to_string(),
            handle: handle,
            dispatcher: dispatcher,
            remote_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), 0),
        }
    }
    fn http_log(&self, _role: &String, _http: &HttpVersion, _method: &Method, _uri: &Uri, _headers: &Headers) {
        debug!("{} {:?} {:?} {:?} {}",_role,_http,_method,_uri,self.remote_addr);
    }
}
impl<D> HasRemoteAddr for Session<D>
    where D: RequestDispatcher
{
    fn remote_addr(&mut self, addr: SocketAddr) {
        self.remote_addr = addr;
    }
}

impl<D> NewService for Session<D>
    where D: RequestDispatcher
{
    type Request = Request;
    type Response = Response;
    type Error = hyper::Error;
    type Instance = Self;
    
    fn new_service(&self) -> Result<Self::Instance,std::io::Error> {
        Ok(self.clone())
    }
}

impl<D> Service for Session<D>
    where D: RequestDispatcher
{
    type Request = Request;
    type Response = Response;
    type Error = hyper::Error;
    type Future = Box<Future<Item=Self::Response, Error=Self::Error>>;

    fn call(&self, req: Request) -> Self::Future {
        let (method,uri,http,headers,body) = req.deconstruct();
        self.http_log(&self.role,&http,&method,&uri,&headers);
        let dispatcher = self.dispatcher.clone();
        let role = self.role.clone();
        let hd = self.dispatcher.http_debug();
        let remote_addr = self.remote_addr;
        let fut = body.concat2()
            .and_then(move |body| {
                match dispatcher.dispatch(&role,&http,&method,&uri,&headers,body.as_ref()) {
                    DispatchResult::Ok(req) => {
                        let (tx_body, rx_body) = fsync::oneshot::channel();
                        dispatcher.process(req,ReplySender {
                            http_debug: hd,
                            sender: ReplySenderVariant::Oneshot(Some(tx_body)),
                        },ClientInfo(remote_addr));
                        Either::B(rx_body          
                                  .or_else(|_| {
                                      warn!("request was cancelled");
                                      future::ok(JsonResponse::empty_error())
                                  })
                                  .and_then(|jr| future::ok(jr.to_response())))
                    },
                    DispatchResult::ChunkedOk(req) => {
                        let (tx_body, rx_body) = fsync::mpsc::channel(10);
                        dispatcher.process(req,ReplySender {
                            http_debug: hd,
                            sender: ReplySenderVariant::Stream(tx_body),
                        },ClientInfo(remote_addr));
                        Either::A(future::ok(JsonResponse::ChunkedOk(rx_body).to_response()))
                    },
                    DispatchResult::Err(jr) => Either::A(future::ok(jr.to_response())),
                }
            });
        Box::new(fut)
    }
}
