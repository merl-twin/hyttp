use hyper::{
    Uri,
    header::{CONTENT_LENGTH,CONTENT_TYPE},
    self,Request,body,Body,
    client::{self,HttpConnector},
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
    connect_timeout: Option<std::time::Duration>, 
}
impl ClientRequest {
    pub fn from_url_and_json<S: Serialize>(url: &str, req: &S) -> Result<ClientRequest,ClientError> {       
        Ok(ClientRequest {
            server_uri: url.parse().map_err(ClientError::Uri)?,
            json: Some(serde_json::to_string(req).map_err(ClientError::Json)?.into_bytes()),
            connect_timeout: None,
        })
    }
    pub fn get_from_url(url: &str) -> Result<ClientRequest,ClientError> {
        Ok(ClientRequest {
            server_uri: url.parse().map_err(ClientError::Uri)?,
            json: None,
            connect_timeout: None,
        })
    }
    pub fn set_connect_timeout(mut self, timeout: std::time::Duration) -> ClientRequest {
        self.connect_timeout = Some(timeout);
        self
    }
}
impl ClientRequest {
    pub async fn future<R: DeserializeOwned>(self) -> Result<R,ClientError> {    
        let client = client::Builder::default().build({
            let mut http_conn = HttpConnector::new();
            http_conn.set_connect_timeout(self.connect_timeout);
            http_conn
        });
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


pub struct Client<D: DeserializeOwned> {
    cli: client::Client<client::HttpConnector>,
    _d: std::marker::PhantomData<D>,
}
impl<D: DeserializeOwned> Client<D> {
    pub fn new() -> Client<D> {
        Client {
            cli: client::Client::new(),
            _d: std::marker::PhantomData,
        }
    }
    pub fn with_connect_timeout(timeout: std::time::Duration) -> Client<D> {
        Client {
            cli: client::Builder::default().build({
                let mut http_conn = HttpConnector::new();
                http_conn.set_connect_timeout(Some(timeout));
                http_conn
            }),
            _d: std::marker::PhantomData,
        }
    }
    pub async fn post_json<S: Serialize>(&self, uri: &str, req: &S) -> Result<D,ClientError> {
        let uri: Uri = uri.parse().map_err(ClientError::Uri)?;
        let json = serde_json::to_string(req).map_err(ClientError::Json)?.into_bytes();
        let req = Request::post(uri)
            .header(CONTENT_TYPE,mime::APPLICATION_JSON.as_ref())
            .header(CONTENT_LENGTH,json.len() as u64)
            .body(Body::from(json))
            .map_err(ClientError::Request)?;
        let response = self.cli.request(req).await.map_err(ClientError::Hyper)?;       
        let buffer = body::to_bytes(response.into_body()).await.map_err(ClientError::Hyper)?;
        serde_json::from_slice(buffer.as_ref()).map_err(ClientError::Json)
    }
    pub async fn get(&self, uri: &str) -> Result<D,ClientError> {
        let uri: Uri = uri.parse().map_err(ClientError::Uri)?;
        let req = Request::get(uri)
            .body(Body::empty())
            .map_err(ClientError::Request)?;
        let response = self.cli.request(req).await.map_err(ClientError::Hyper)?;       
        let buffer = body::to_bytes(response.into_body()).await.map_err(ClientError::Hyper)?;
        serde_json::from_slice(buffer.as_ref()).map_err(ClientError::Json)
    }
}

