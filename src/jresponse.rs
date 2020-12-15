use log::{debug,error,warn};
use serde::{Serialize,Deserialize};

use hyper::{
    header::{CONTENT_LENGTH,CONTENT_TYPE},
    Response, Body,
    StatusCode,
};

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

enum ReplyValue {
    Reply(Reply),
    Value(Value),
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
    ChunkedOk(Body),
    Raw{ status: StatusCode, value: Value }, // raw JSON - kind of 'any' reply variant
}
impl JsonResponse {
    pub fn ok(value: Value) -> JsonResponse {
        JsonResponse::Ok(value)
    }
    pub fn raw(status: StatusCode, value: Value) -> JsonResponse {
        JsonResponse::Raw{ status, value }
    }
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
    fn reply_value(self) -> Result<ReplyValue,()> {
        Ok(ReplyValue::Reply(match self {
            JsonResponse::NotFound =>  Reply::new("Not Found".to_string(),None),
            JsonResponse::MethodNotAllowed => Reply::new("Method Not Allowed".to_string(),None),
            JsonResponse::BadRequest => Reply::new("Bad Request".to_string(),None),
            JsonResponse::TooManyRequests => Reply::new("Too Many Requests".to_string(),None),
            JsonResponse::EmptyQuery => Reply::new("Empty Query".to_string(),None),
            JsonResponse::RequestTimeout => Reply::new("Request Timeout".to_string(),None),
            JsonResponse::InternalServerError(v) => Reply::new("Internal Server Error".to_string(),v),
            JsonResponse::Ok(v) => Reply::new("Ok".to_string(),Some(v)),
            JsonResponse::ChunkedOk(..) => return Err(()),
            JsonResponse::Raw{ value, .. }  => return Ok(ReplyValue::Value(value)),
        }))
    }
    fn status(&self) -> StatusCode {
        match *self {
            JsonResponse::NotFound => StatusCode::NOT_FOUND,
            JsonResponse::MethodNotAllowed => StatusCode::METHOD_NOT_ALLOWED,
            JsonResponse::TooManyRequests => StatusCode::TOO_MANY_REQUESTS,
            JsonResponse::BadRequest | 
            JsonResponse::EmptyQuery => StatusCode::BAD_REQUEST,
            JsonResponse::RequestTimeout => StatusCode::REQUEST_TIMEOUT,
            JsonResponse::InternalServerError(..) => StatusCode::INTERNAL_SERVER_ERROR,
            JsonResponse::Ok(..) |
            JsonResponse::ChunkedOk(..) => StatusCode::OK,
            JsonResponse::Raw{ status, .. } => status,
        }
    }
    pub fn to_response(self) -> Response<Body> {
        fn error() -> Response<Body> {
            let json = "{ \"status\": \"Internal Server Error\" }";
            Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .header(CONTENT_TYPE,mime::APPLICATION_JSON.as_ref())
                .header(CONTENT_LENGTH,json.len() as u64)
                .body(Body::from(json))
                .unwrap_or_else(|_|{
                    warn!("Error generating response (json-error)");
                    let mut response = Response::new(Body::empty());
                    *response.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
                    response
                })
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
            JsonResponse::Ok(..) |
            JsonResponse::Raw{..} => {
                if let Ok(rep_val) = self.reply_value() {
                    match match &rep_val {
                        ReplyValue::Reply(rep) => serde_json::to_string(rep),
                        ReplyValue::Value(val) => serde_json::to_string(val),
                    } {
                        Ok(json) => {
                            debug!("Size: {}KB",json.len()/1024);
                            return Response::builder()
                                .status(status)
                                .header(CONTENT_TYPE,mime::APPLICATION_JSON.as_ref())
                                .header(CONTENT_LENGTH,json.len() as u64)
                                .body(Body::from(json))
                                .unwrap_or_else(|_| {
                                    warn!("Error generating response (json)");
                                    let mut response = Response::new(Body::empty());
                                    *response.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
                                    response
                                })
                        },
                        Err(e) => error!("Json error: {:?}",e),
                    }
                }
                error()
            },
            JsonResponse::ChunkedOk(body) => {
                Response::builder()
                    .status(status)
                    .header(CONTENT_TYPE,mime::APPLICATION_OCTET_STREAM.as_ref())
                    .body(body)
                    .unwrap_or_else(|_| {
                        warn!("Error generating response (chunked-json)");
                        let mut response = Response::new(Body::empty());
                        *response.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
                        response
                    })
            },
        }
    }
}
impl Into<Response<Body>> for JsonResponse {
    fn into(self) -> Response<Body> {
        self.to_response()
    }
}

