use log::{debug,error,warn};
use serde::{Serialize,Deserialize,de::DeserializeOwned};

use hyper::{
    header::{CONTENT_LENGTH,CONTENT_TYPE},
    Response, Body,
    StatusCode,
};

use serde_json::{self,Value};

#[derive(Debug,Serialize,Deserialize)]
pub struct BasicReply {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply: Option<Value>,
}
impl BasicReply {
    pub fn new(s: String, d: Option<Value>) -> BasicReply {
        BasicReply {
            status: s,
            reply: d,
        }
    }
}
impl ApiReply for BasicReply {
    fn error(status: String, data: Option<Value>) -> Self {
        BasicReply::new(status,data)
    }
}

pub trait ApiReply: Sized + Send + std::fmt::Debug + Serialize + DeserializeOwned {
    fn error(status: String, data: Option<Value>) -> Self;
}

#[derive(Debug)]
pub enum JsonResponse<R: ApiReply = BasicReply> {
    NotFound,
    MethodNotAllowed,
    BadRequest,
    TooManyRequests,
    InternalServerError(Option<Value>),
    EmptyQuery,
    RequestTimeout,
    Locked,
    ImATeapot,
    MisdirectedRequest,
    UnprocessableEntity,   
    Ok(R),
}
impl<R: ApiReply> JsonResponse<R> {
    pub fn ok(value: R) -> JsonResponse<R> {
        JsonResponse::Ok(value)
    }
    pub fn internal_error(er: &str) -> JsonResponse<R> {
        match serde_json::to_value(er) {
            Ok(v) => JsonResponse::InternalServerError(Some(v)),
            Err(e) => {
                error!("JsonResponse convert error: {:?}",e);
                JsonResponse::InternalServerError(None)
            }
        }
    }
    pub fn empty_error() -> JsonResponse<R> {
        JsonResponse::InternalServerError(None)
    }
    pub fn reply_value(self) -> R {
        match self {
            JsonResponse::NotFound =>  R::error("Not Found".to_string(),None),
            JsonResponse::MethodNotAllowed => R::error("Method Not Allowed".to_string(),None),
            JsonResponse::BadRequest => R::error("Bad Request".to_string(),None),
            JsonResponse::TooManyRequests => R::error("Too Many Requests".to_string(),None),
            JsonResponse::EmptyQuery => R::error("Empty Query".to_string(),None),
            JsonResponse::RequestTimeout => R::error("Request Timeout".to_string(),None),
            JsonResponse::Locked => R::error("Locked".to_string(),None),
            JsonResponse::ImATeapot => R::error("I'm a teapot".to_string(),None),
            JsonResponse::MisdirectedRequest => R::error("Misdirected Request".to_string(),None),
            JsonResponse::UnprocessableEntity => R::error("Unprocessable Entity".to_string(),None),
            JsonResponse::InternalServerError(v) => R::error("Internal Server Error".to_string(),v),
            JsonResponse::Ok(v) => v,
        }
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
            JsonResponse::Locked => StatusCode::LOCKED,
            JsonResponse::ImATeapot => StatusCode::IM_A_TEAPOT,
            JsonResponse::MisdirectedRequest => StatusCode::MISDIRECTED_REQUEST,
            JsonResponse::UnprocessableEntity => StatusCode::UNPROCESSABLE_ENTITY,
            JsonResponse::Ok(..) => StatusCode::OK,
        }
    }
    pub fn to_response(self) -> Response<Body> {
        fn error<R: ApiReply>() -> Response<Body> {
            let r = JsonResponse::<R>::InternalServerError(None).reply_value();
            if let Ok(json) = serde_json::to_string(&r) {
                if let Ok(r) = Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .header(CONTENT_TYPE,mime::APPLICATION_JSON.as_ref())
                    .header(CONTENT_LENGTH,json.len() as u64)
                    .body(Body::from(json)) {
                        return r;                            
                    }
            }

            warn!("Error generating response (json-error)");
            let mut response = Response::new(Body::empty());
            *response.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
            response
        }

        let status = self.status();
        match serde_json::to_string(&self.reply_value()) {
            Ok(json) => {
                debug!("Size: {}KB",json.len()/1024);
                Response::builder()
                    .status(status)
                    .header(CONTENT_TYPE,mime::APPLICATION_JSON.as_ref())
                    .header(CONTENT_LENGTH,json.len() as u64)
                    .body(Body::from(json))
                    .unwrap_or_else(|_| {
                        warn!("Error generating response (json)");
                        error::<R>()
                    })
            },
            Err(e) => {
                error!("Json error: {:?}",e);
                error::<R>()
            },
        }
    }
}
impl<R: ApiReply> Into<Response<Body>> for JsonResponse<R> {
    fn into(self) -> Response<Body> {
        self.to_response()
    }
}

