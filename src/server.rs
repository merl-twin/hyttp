use futures::{
    future,
    stream,
    channel::oneshot,
    StreamExt,
    TryFutureExt,
};
use log::{debug,info,error,warn};
use hyper::{
    Method,Uri,Version,StatusCode, HeaderMap,
    Request, Response, Body,
    header, body,
    service::service_fn,
    server::conn::Http,
};


use serde_json;
use serde::Serialize;

use std::{
    self,
    fmt::Debug,
    sync::{atomic,Arc},
    net::SocketAddr,
    collections::BTreeMap,
    convert::Infallible,
};

use tokio::{
    net::TcpListener,
    runtime::Handle,
};

use crate::jresponse::JsonResponse;
use crate::qstring;

pub struct HtmlSender(oneshot::Sender<String>);
impl HtmlSender {
    pub fn send(self, s: String) -> Result<(),FrontendError> {
        self.0.send(s).map_err(|_| FrontendError::BackendSend)
    }
}

enum ReplySenderVariant {
    Oneshot(Option<oneshot::Sender<JsonResponse>>),
    Stream(body::Sender),
}

pub struct ReplySender {
    http_debug: bool,
    sender: ReplySenderVariant,
}
impl ReplySender {
    pub fn oneshot() -> (ReplySender,oneshot::Receiver<JsonResponse>) {
        let (tx_body, rx_body) = oneshot::channel();
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
    _destructor: oneshot::Sender<()>,
    handle: Handle,
}
impl ReactorControl {
    pub fn handle(&self) -> Handle {
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
    type Request: Send;    
    fn dispatch(&self, role: &str, http: &Version, method: &Method, uri: &Uri, headers: &HeaderMap, body: &[u8]) -> DispatchResult<Self::Request>;
    fn process(&self, req: Self::Request, sender: ReplySender, info: ClientInfo);

    fn process_html(&self, _req: Self::Request, sender: HtmlSender, _info: ClientInfo) {
        sender.send("".to_string()).ok();
    }
    fn reactor_control(&mut self, _ctrl: ReactorControl) -> InitDestructor {
        InitDestructor::Ignore
    }
    //fn connection_error_reporter(&self, _error: FrontendError) {}
    fn http_debug(&self) -> bool { false }
    fn http_log(&self, role: &String, http: &Version, method: &Method, uri: &Uri, _headers: &HeaderMap, remote_addr: SocketAddr) {
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
    Tokio(std::io::Error),
    Tcp(std::io::Error),
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
    pub fn run<D: RequestDispatcher + Send>(self, mut dispatcher: D) -> Result<(),FrontendError> {
        let mut executor = tokio::runtime::Runtime::new().map_err(FrontendError::Tokio)?;
        let (tx,destructor) = oneshot::channel::<()>();
        let init_destructor = dispatcher.reactor_control(ReactorControl{
            _destructor: tx,
            handle: executor.handle().clone(),
        });

        let process = match init_destructor {
            InitDestructor::Ignore => None,
            InitDestructor::Init => {
                let process = Arc::new(atomic::AtomicBool::new(true));
                executor.spawn({
                    let process = process.clone();
                    async move {
                        destructor.await.ok();
                        process.store(false,atomic::Ordering::Relaxed);
                    }
                });
                Some(process)
            },
        };

        let handle = executor.handle().clone();
        executor.block_on(self.listen(dispatcher, process, handle).and_then(|()| future::err(FrontendError::UnexpectedTermination)))
    }
    async fn listen<D: RequestDispatcher + Send>(self, dispatcher: D, process: Option<Arc<atomic::AtomicBool>>, handle: Handle) -> Result<(),FrontendError> {
        let mut vs = Vec::new();
        for (alias,addr) in self.addresses {    
            info!("starting '{}' server: {}",alias,addr);
            vs.push(TcpListener::bind(addr).await
                    .map_err(FrontendError::Tcp)?
                    .map(move |conn| (alias.clone(),conn)));
        }
        let mut clients = stream::select_all(vs);
        let mut n: usize = 0;
        loop {
            if let Some(process) = &process {
                if !process.load(atomic::Ordering::Relaxed) {
                    break
                }
            }
            n += 1;
            match clients.next().await {
                None => { error!("[tcp] listeners failed"); break; },
                Some((alias,Err(e))) => {
                    error!("[{}]-({}) Invalid connection: {:?}",alias,n,e);
                    continue;
                },
                Some((alias,Ok(conn))) => {
                    let remote = match conn.peer_addr() {
                        Err(e) => { error!("[{}]-({}) Invalid connection peer addr: {:?}",alias,n,e); continue; },
                        Ok(addr) => addr,                    
                    };
                    handle.spawn({                                      
                        Http::new().serve_connection(conn,service_fn({
                            let dispatcher = dispatcher.clone();   
                            let alias = alias.clone();
                            move |request| {
                                let dispatcher = dispatcher.clone();
                                let alias = alias.clone();
                                async move {
                                    Ok::<_, Infallible>(service_call(alias,remote,request,dispatcher).await)
                                }
                            }
                        }))
                            .map_err(move |e| {
                                error!("[{}]-({}) Invalid connection processing: {:?}",alias,n,e);
                            })
                    }); // detaching
                }
            }
        }
        /*stream::select_all(vs).for_each(move |(alias,conn)| {
            let dispatcher = dispatcher.clone();
            let process = process.clone();
            async move {
                let conn = match conn {
                    Ok(conn) => conn,
                    Err(e) => {
                        error!("[tcp] Invalid connection: {:?}",e);
                        return ();
                    },
                };
                let remote = match conn.peer_addr() {
                    Ok(addr) => addr,
                    Err(e) => {
                        error!("[tcp] Invalid connection peer addr: {:?}",e);
                        return ();
                    },
                };           
                if let Err(e) = Http::new().serve_connection(conn,service_fn(move |request| {
                    let dispatcher = dispatcher.clone();
                    let alias = alias.clone();
                    async move {
                        Ok::<_, Infallible>(service_call(alias,remote,request,dispatcher).await)
                    }
                })).await {
                    error!("[tcp] Invalid connection processing: {:?}",e);
                }
            }
        }).await;*/
        Ok(())
    }
}

async fn service_call<D: RequestDispatcher>(role: String, remote_addr: SocketAddr, req: Request<Body>, dispatcher: D) -> Response<Body> {
    fn html_reply(s: String) -> Response<Body> {
        Response::builder()
            .status(StatusCode::OK)
            .header(header::CACHE_CONTROL,"no-cache")
            .header(header::CONTENT_TYPE,mime::TEXT_HTML_UTF_8.as_ref())
            .header(header::CONTENT_LENGTH,s.len() as u64)
            .body(Body::from(s))
            .unwrap_or({
                warn!("Error generating response (html)");
                let mut response = Response::new(Body::empty());
                *response.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
                response
            })
    }

    
    info!("Incomming connection: {}",remote_addr);
    let (req_parts, req_body) = req.into_parts();
    let method = req_parts.method;
    let uri = req_parts.uri;
    let version = req_parts.version;
    let headers = req_parts.headers;
    //extensions
    dispatcher.http_log(&role,&version,&method,&uri,&headers,remote_addr);
    let http_debug = dispatcher.http_debug();
    match body::to_bytes(req_body).await {
        Err(e) => {
            error!("failed to get request body: {:?}",e);
            match http_debug {
                true => JsonResponse::internal_error(&format!("failed to get request body: {:?}",e)),
                false => JsonResponse::empty_error(),
            }.to_response()
        },
        Ok(bytes) => {
            let dspr = dispatcher.dispatch(&role,&version,&method,&uri,&headers,bytes.as_ref());
            match dspr {
                DispatchResult::Html(req) => {
                    let (tx_body, rx_body) = oneshot::channel();
                    dispatcher.process_html(req,HtmlSender(tx_body),ClientInfo(remote_addr));
                    html_reply(rx_body.await.unwrap_or("".to_string()))
                },
                DispatchResult::Ok(req) => {
                    let (tx_body, rx_body) = oneshot::channel();
                    dispatcher.process(req,ReplySender {
                        http_debug: http_debug,
                        sender: ReplySenderVariant::Oneshot(Some(tx_body)),
                    },ClientInfo(remote_addr));
                    rx_body.await
                        .map_err(|_| warn!("request was cancelled"))
                        .unwrap_or(JsonResponse::empty_error())
                        .to_response()
                },
                DispatchResult::ChunkedOk(req) => {
                    let (tx_body, rx_body) = Body::channel();
                    dispatcher.process(req,ReplySender {
                        http_debug: http_debug,
                        sender: ReplySenderVariant::Stream(tx_body),
                    },ClientInfo(remote_addr));
                    JsonResponse::ChunkedOk(rx_body).to_response()
                },
                DispatchResult::Err(jr) => jr.to_response(),
            }
        },
    }
}
