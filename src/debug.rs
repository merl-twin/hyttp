use async_trait::async_trait;

use std::sync::{Arc,Mutex};
use serde::{Serialize,de::DeserializeOwned};

use crate::*;

#[async_trait]
pub trait DebugServer: Sync + Send + Clone + 'static {
    type Request: Send + Serialize + DeserializeOwned;
    type Reply: ApiReply;

    async fn process(&self, role: String, req: Self::Request) -> Self::Reply;

    fn client() -> client::Client<Self::Reply> {
        client::Client::new()
    }
}

#[derive(Clone)]
pub struct Server<S: DebugServer> {
    ctrl: Option<Arc<Mutex<Option<ReactorControl>>>>,
    runtime: Option<tokio::runtime::Handle>,
    ds: Arc<S>,
}
impl<S: DebugServer> Server<S> {
    pub fn new<>(ds: S) -> Server<S> {
        Server {
            ctrl: None,
            runtime: None,
            ds: Arc::new(ds),
        }
    }
}
pub enum AppRequest<R> {
    Ready,
    Exit,
    Payload{ role: String, request: R },
}
impl<S: DebugServer> RequestDispatcher for Server<S> {
    type Request = AppRequest<S::Request>;
    type Reply = S::Reply;
    
    fn dispatch(&self, role: &str, _http: &Version, method: &Method, uri: &Uri, _headers: &header::HeaderMap, body: &[u8]) -> DispatchResult<Self::Request,Self::Reply> {
        match (method, uri.path()) {
            (&Method::POST, "/") => match serde_json::from_slice::<S::Request>(body) {
                Ok(req) => DispatchResult::Ok(AppRequest::Payload{ role: role.to_string(), request: req }),
                Err(e) => DispatchResult::Err(JsonResponse::internal_error(&format!("{:?}",e))),
            },
            (&Method::GET, "/ready") => DispatchResult::Ok(AppRequest::Ready),
            (&Method::GET, "/exit") => DispatchResult::Ok(AppRequest::Exit),
            _ => DispatchResult::Err(JsonResponse::MethodNotAllowed),
        }
    }
    fn process(&self, req: Self::Request, mut sender: ReplySender<Self::Reply>, _info: ClientInfo) {
        let (role,req) = match req {
            AppRequest::Ready => {
                sender.ok(<Self::Reply as ApiReply>::error("Ok".to_string(),None)).ok();
                return;
            },
            AppRequest::Exit => {
                sender.ok(<Self::Reply as ApiReply>::error("Ok".to_string(),None)).ok();
                if let Some(ctrl) = &self.ctrl {
                    std::mem::drop(ctrl.lock().unwrap().take());
                }
                return;
            },
            AppRequest::Payload{ role, request } => (role,request),
        };
        let ds = self.ds.clone();
        if let Some(handle) = &self.runtime {
            handle.spawn(async move {
                sender.ok(ds.process(role,req).await).ok();
            });
        }
    }
    fn reactor_control(&mut self, ctrl: ReactorControl) -> InitDestructor {
        self.runtime = Some(ctrl.handle());
        self.ctrl = Some(Arc::new(Mutex::new(Some(ctrl))));
        InitDestructor::Init
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Serialize,Deserialize};
    use serde_json::Value;
    
    #[derive(Clone)]
    struct BasicServer;
    impl BasicServer {
        fn new() -> BasicServer { BasicServer }
    }
    #[derive(Debug,Serialize,Deserialize)]
    struct BasicResult {
        status: String,
        payload: String,
    }
    impl ApiReply for BasicResult {
        fn error(status: String, data: Option<Value>) -> Self {
            BasicResult {
                status,
                payload: match data {
                    None | Some(Value::Null) => String::new(),
                    Some(Value::String(s)) => s,
                    Some(v) => v.to_string(), 
                },
            }
        }
    }
    #[async_trait]
    impl DebugServer for BasicServer {
        type Request = u64;
        type Reply = BasicResult;
        
        async fn process(&self, role: String, req: Self::Request) -> Self::Reply {
            match &role as &str {
                "back_x1" => BasicResult{ status: "Ok".to_string(), payload: format!("{}: {}",role,req) },
                "back_x2" => BasicResult{ status: "Ok".to_string(), payload: format!("{}: {}",role,req+1) },
                "back_x3" => BasicResult{ status: "Ok".to_string(), payload: format!("{}: {}",role,req+2) },
                _ => BasicResult{ status: "Ok".to_string(), payload: format!("{}: ???",role) },
            }
        }
    }
    
    #[test]
    fn basic() {
        /*simple_logger::SimpleLogger::new()
        .with_level(log::LevelFilter::Info)
        .with_module_level("http",log::LevelFilter::Debug)
        .init().unwrap();
         */
        let mut runtime = tokio::runtime::Runtime::new().unwrap();
        
        let dispatcher = Server::new(BasicServer::new());
        let mut front = FrontendBuilder::new();
        front = front.add_role("back_x1","127.0.0.1:12001").unwrap();
        front = front.add_role("back_x2","127.0.0.1:12002").unwrap();
        front = front.add_role("back_x3","127.0.0.1:12003").unwrap();
        
        runtime.spawn(front.async_run(dispatcher,runtime.handle().clone()));
        
        runtime.block_on(async {
            let cli = BasicServer::client();
            loop {
                match cli.get("http://127.0.0.1:12001/ready").await {
                    Ok(br) if br.status == "Ok" => { break; },
                    Ok(br) => log::warn!("Not ready: {:?}",br),
                    Err(e) => log::warn!("Not ready: {:?}",e),
                }
                log::warn!("waiting for server...");
                tokio::time::delay_for(tokio::time::Duration::new(0,250_000_000)).await;
            }
            assert_eq!(&cli.post_json("http://127.0.0.1:12001/",&56u64).await.unwrap().payload,"back_x1: 56");
            assert_eq!(&cli.post_json("http://127.0.0.1:12001/",&88783475u64).await.unwrap().payload,"back_x1: 88783475");
            assert_eq!(&cli.post_json("http://127.0.0.1:12002/",&56u64).await.unwrap().payload,"back_x2: 57");
            assert_eq!(&cli.post_json("http://127.0.0.1:12002/",&88783475u64).await.unwrap().payload,"back_x2: 88783476");
            assert_eq!(&cli.post_json("http://127.0.0.1:12003/",&56u64).await.unwrap().payload,"back_x3: 58");
            assert_eq!(&cli.post_json("http://127.0.0.1:12003/",&88783475u64).await.unwrap().payload,"back_x3: 88783477");
        });
        runtime.block_on(client::ClientRequest::get_from_url("http://127.0.0.1:12001/exit").unwrap().future::<BasicReply>()).ok();
        //panic!();
    }
    
}
