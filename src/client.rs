use hyper::{
    Uri,
    header::{CONTENT_LENGTH,CONTENT_TYPE},
    self,client,Request,body,Body,
};
use mime;
use serde::{Serialize,de::DeserializeOwned};

#[derive(Debug)]
pub enum ClientError {
    Json(serde_json::Error),
    Uri(hyper::http::uri::InvalidUri),
    Request(hyper::http::Error),
    Hyper(hyper::Error),
}

pub struct ClientRequest {
    server_uri: Uri,
    json: Option<Vec<u8>>,
}
impl ClientRequest {
    pub fn from_url_and_json<S: Serialize>(url: &str, req: &S) -> Result<ClientRequest,ClientError> {       
        Ok(ClientRequest {
            server_uri: url.parse().map_err(ClientError::Uri)?,
            json: Some(serde_json::to_string(req).map_err(ClientError::Json)?.into_bytes()),
        })
    }
    pub fn get_from_url(url: &str) -> Result<ClientRequest,ClientError> {
        Ok(ClientRequest {
            server_uri: url.parse().map_err(ClientError::Uri)?,
            json: None,
        })
    }
}
impl ClientRequest {
    pub async fn future<R: DeserializeOwned>(self) -> Result<R,ClientError> {
        let client = client::Client::new();
        let req = match self.json {
            Some(json) => Request::post(self.server_uri)
                .header(CONTENT_TYPE,mime::APPLICATION_JSON.as_ref())
                .header(CONTENT_LENGTH,json.len() as u64)
                .body(Body::from(json)),
            None => Request::get(self.server_uri).body(Body::empty()),
        }.map_err(ClientError::Request)?;


        let response = client.request(req).await.map_err(ClientError::Hyper)?;       
        let buffer = body::to_bytes(response.into_body()).await.map_err(ClientError::Hyper)?;

        serde_json::from_slice(buffer.as_ref()).map_err(ClientError::Json)
    }
    pub fn server_uri(&self) -> &Uri {
        &self.server_uri
    }
}


