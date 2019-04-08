use futures::sync;

use hyper;
use hyper::header::{ContentLength,ContentType};
use hyper::server::Response;
use hyper::{Chunk,StatusCode};

use serde_json::{self,Value};

#[derive(Serialize,Deserialize)]
struct Reply {
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    reply: Option<Value>,
}
impl Reply {
    fn new(s: String, d: Option<Value>) -> Reply {
        Reply {
            status: s,
            reply: d,
        }
    }
}

#[derive(Debug)]
pub enum JsonResponse {
    NotFound,
    MethodNotAllowed,
    BadRequest,
    TooManyRequests,
    InternalServerError(Option<Value>),
    EmptyQuery,
    RequestTimeout,
    Ok(Value),
    ChunkedOk(sync::mpsc::Receiver<Result<Chunk,hyper::Error>>),
}
impl JsonResponse {
    pub fn internal_error(er: &str) -> JsonResponse {
        match serde_json::to_value(er) {
            Ok(v) => JsonResponse::InternalServerError(Some(v)),
            Err(e) => {
                error!("JsonResponse convert error: {:?}",e);
                JsonResponse::InternalServerError(None)
            }
        }
    }
    pub fn empty_error() -> JsonResponse {
        JsonResponse::InternalServerError(None)
    }
    fn reply(self) -> Result<Reply,()> {
        Ok(match self {
            JsonResponse::NotFound =>  Reply::new("Not Found".to_string(),None),
            JsonResponse::MethodNotAllowed => Reply::new("Method Not Allowed".to_string(),None),
            JsonResponse::BadRequest => Reply::new("Bad Request".to_string(),None),
            JsonResponse::TooManyRequests => Reply::new("Too Many Requests".to_string(),None),
            JsonResponse::EmptyQuery => Reply::new("Empty Query".to_string(),None),
            JsonResponse::RequestTimeout => Reply::new("Request Timeout".to_string(),None),
            JsonResponse::InternalServerError(v) => Reply::new("Internal Server Error".to_string(),v),
            JsonResponse::Ok(v) => Reply::new("Ok".to_string(),Some(v)),
            JsonResponse::ChunkedOk(..) => return Err(()),
        })
    }
    fn status(&self) -> StatusCode {
        match *self {
            JsonResponse::NotFound => StatusCode::NotFound,
            JsonResponse::MethodNotAllowed => StatusCode::MethodNotAllowed,
            JsonResponse::TooManyRequests => StatusCode::TooManyRequests,
            JsonResponse::BadRequest | 
            JsonResponse::EmptyQuery => StatusCode::BadRequest,
            JsonResponse::RequestTimeout => StatusCode::RequestTimeout,
            JsonResponse::InternalServerError(..) => StatusCode::InternalServerError,
            JsonResponse::Ok(..) |
            JsonResponse::ChunkedOk(..) => StatusCode::Ok,
        }
    }
    pub fn to_response(self) -> Response {
        fn error() -> Response {
            let json = "{ \"status\": \"Internal Server Error\" }";
            Response::new()
                .with_status(StatusCode::InternalServerError)
                .with_header(ContentType(hyper::mime::APPLICATION_JSON))
                .with_header(ContentLength(json.len() as u64))
                .with_body(json)
        }
        
        let status = self.status();
        match self {
            JsonResponse::NotFound |
            JsonResponse::MethodNotAllowed |
            JsonResponse::TooManyRequests |
            JsonResponse::BadRequest |
            JsonResponse::EmptyQuery |
            JsonResponse::RequestTimeout | 
            JsonResponse::InternalServerError(..) |
            JsonResponse::Ok(..) => {
                if let Ok(rep) = self.reply() {
                    match serde_json::to_string(&rep) {
                        Ok(json) => {
                            debug!("Size: {}KB",json.len()/1024);
                            return Response::new()
                                .with_status(status)
                                .with_header(ContentType(hyper::mime::APPLICATION_JSON))
                                .with_header(ContentLength(json.len() as u64))
                                .with_body(json);
                        },
                        Err(e) => error!("Json error: {:?}",e),
                    }
                }
                error()
            },
            JsonResponse::ChunkedOk(receiver) => {
                Response::new()
                    .with_status(status)
                    .with_header(ContentType(hyper::mime::APPLICATION_OCTET_STREAM))
                    .with_body(receiver)
            },
        }
    }
}
impl Into<Response> for JsonResponse {
    fn into(self) -> Response {
        self.to_response()
    }
}

