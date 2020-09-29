use futures::future::{self,Future,Either};
use futures::{
    Canceled,
    sync as fsync,
    Stream,
};

use hyper;
pub use hyper::{
    Chunk,Method,Headers,Uri,HttpVersion,StatusCode,
    server::{Http, Request, Response, Service, NewService, RemoteAddr, HasRemoteAddr},
    header::{ContentLength,ContentType,CacheControl,CacheDirective},
};

use tokio_core::reactor::{Core, Handle, Remote};

use serde_json;
use serde::Serialize;

use std::{
    self,
    fmt::Debug,
    sync::Arc,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    collections::BTreeMap,
    boxed::Box,
};
use crate::jresponse::JsonResponse;
use crate::qstring;

pub struct HtmlSender(fsync::oneshot::Sender<String>);
impl HtmlSender {
    pub fn send(self, s: String) -> Result<(),FrontendError> {
        self.0.send(s).map_err(|_| FrontendError::BackendSend)
    }
}

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
    pub fn error(&mut self, se: String) -> Result<(),FrontendError> {
        match &mut self.sender {
            ReplySenderVariant::Oneshot(sender) => match sender.take() {
                Some(sender) => match self.http_debug {
                    true => sender.send(JsonResponse::internal_error(&se)).map_err(|_| FrontendError::BackendSend),
                    false => sender.send(JsonResponse::empty_error()).map_err(|_| FrontendError::BackendSend),
                },
                None => Err(FrontendError::NoSender),
            },
            ReplySenderVariant::Stream(_sender) => Err(FrontendError::NotImplemented),
        }
    }
    pub fn send_result<E: Debug, S: Serialize>(&mut self, rr: Result<S,E>) -> Result<(),FrontendError> {
        match &mut self.sender {
            ReplySenderVariant::Oneshot(sender) => match sender.take() {
                Some(sender) => {
                    match (self.http_debug, rr) {
                        (_,Ok(s)) => match (self.http_debug,serde_json::to_value(&s)) {
                            (_,Ok(r)) => sender.send(JsonResponse::Ok(r)).map_err(|_| FrontendError::BackendSend),
                            (true,Err(e)) => sender.send(JsonResponse::internal_error(&format!("{:?}",e))).map_err(|_| FrontendError::BackendSend),
                            (false,Err(_)) => sender.send(JsonResponse::empty_error()).map_err(|_| FrontendError::BackendSend),
                        },
                        (true,Err(e)) => sender.send(JsonResponse::internal_error(&format!("{:?}",e))).map_err(|_| FrontendError::BackendSend),
                        (false,Err(_)) => sender.send(JsonResponse::empty_error()).map_err(|_| FrontendError::BackendSend),
                    }
                },
                None => Err(FrontendError::NoSender),
            },
            ReplySenderVariant::Stream(_sender) => Err(FrontendError::NotImplemented),
        }
    }
}
pub enum DispatchResult<R> {
    Ok(R),
    ChunkedOk(R),
    Html(R),
    Err(JsonResponse),
}
impl<R> From<qstring::UrlParseError> for DispatchResult<R> {
    fn from(err: qstring::UrlParseError) -> DispatchResult<R> {
        DispatchResult::Err(JsonResponse::internal_error(&format!("{:?}",err)))
    }
}

pub struct ReactorControl {
    _destructor: fsync::oneshot::Sender<()>,
    handle: Remote,
}
impl ReactorControl {
    pub fn handle(&self) -> Remote {
        self.handle.clone()
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

    fn process_html(&self, _req: Self::Request, sender: HtmlSender, _info: ClientInfo) {
        sender.send("".to_string()).ok();
    }
    fn reactor_control(&mut self, _ctrl: ReactorControl) -> InitDestructor {
        InitDestructor::Ignore
    }
    fn connection_error_reporter(&self, _error: FrontendError) {}
    fn http_debug(&self) -> bool { false }
    fn http_log(&self, role: &String, http: &HttpVersion, method: &Method, uri: &Uri, _headers: &Headers, remote_addr: SocketAddr) {
        debug!("{} {:?} {:?} {:?} {}",role,http,method,uri,remote_addr);
    }
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
            handle: core.handle().remote().clone(),
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
                    handle.spawn(conn.map_err(move |e| dsp.connection_error_reporter(FrontendError::Hyper(e))));
                    Ok(())
                }))
            },
            2 => {
                let dsp = shared_d.clone();
                Either::B(vs.pop().unwrap().select(vs.pop().unwrap()).for_each(move |conn| {
                    let dsp = dsp.clone();
                    info!("Incomming connection: {}",conn.remote());
                    handle.spawn(conn.map_err(move |e| dsp.connection_error_reporter(FrontendError::Hyper(e))));
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
    fn http_log(&self, role: &String, http: &HttpVersion, method: &Method, uri: &Uri, headers: &Headers) {
        self.dispatcher.http_log(role,http,method,uri,headers,self.remote_addr);
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
    type Future = Box<dyn Future<Item=Self::Response, Error=Self::Error>>;

    fn call(&self, req: Request) -> Self::Future {
        fn html_reply(s: String) -> Response {
            Response::new()
                .with_status(StatusCode::Ok)
                .with_header(CacheControl(vec![CacheDirective::NoCache]))
                .with_header(ContentType(hyper::mime::TEXT_HTML_UTF_8))
                .with_header(ContentLength(s.len() as u64))
                .with_body(s)
        }
        
        let (method,uri,http,headers,body) = req.deconstruct();
        self.http_log(&self.role,&http,&method,&uri,&headers);
        let dispatcher = self.dispatcher.clone();
        let role = self.role.clone();
        let hd = self.dispatcher.http_debug();
        let remote_addr = self.remote_addr;
        let fut = body.concat2()
            .and_then(move |body| {
                match dispatcher.dispatch(&role,&http,&method,&uri,&headers,body.as_ref()) {
                    DispatchResult::Html(req) => {
                        let (tx_body, rx_body) = fsync::oneshot::channel();
                        dispatcher.process_html(req,HtmlSender(tx_body),ClientInfo(remote_addr));
                        Either::A(rx_body
                                  .and_then(|s| future::ok(html_reply(s)))
                                  .or_else(|_| future::ok(html_reply("".to_string()))))
                    },
                    DispatchResult::Ok(req) => {
                        let (tx_body, rx_body) = fsync::oneshot::channel();
                        dispatcher.process(req,ReplySender {
                            http_debug: hd,
                            sender: ReplySenderVariant::Oneshot(Some(tx_body)),
                        },ClientInfo(remote_addr));
                        Either::B(
                            Either::B(rx_body          
                                      .or_else(|_| {
                                          warn!("request was cancelled");
                                          future::ok(JsonResponse::empty_error())
                                      })
                                      .and_then(|jr| future::ok(jr.to_response()))))
                    },
                    DispatchResult::ChunkedOk(req) => {
                        let (tx_body, rx_body) = fsync::mpsc::channel(10);
                        dispatcher.process(req,ReplySender {
                            http_debug: hd,
                            sender: ReplySenderVariant::Stream(tx_body),
                        },ClientInfo(remote_addr));
                        Either::B(
                            Either::A(future::ok(JsonResponse::ChunkedOk(rx_body).to_response())))
                    },
                    DispatchResult::Err(jr) => Either::B(Either::A(future::ok(jr.to_response()))),
                }
            });
        Box::new(fut)
    }
}
