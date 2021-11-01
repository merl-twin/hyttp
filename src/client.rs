use hyper::{
    Uri,
    header::{CONTENT_LENGTH,CONTENT_TYPE},
    self,Request,body,Body,Response,
    client::{self,HttpConnector,ResponseFuture},
    
};
use mime;
use serde::{Serialize,de::DeserializeOwned};

#[derive(Debug)]
pub enum ClientError {
    Json(serde_json::Error),
    Uri(hyper::http::uri::InvalidUri),
    Request(hyper::http::Error),
    Hyper(hyper::Error),
    Timeout(String),
}

pub struct ClientRequest {
    server_uri: Uri,
    json: Option<Vec<u8>>,
    connect_timeout: Option<std::time::Duration>,
    request_timeout: Option<std::time::Duration>, 
}
impl ClientRequest {
    pub fn from_url_and_json<S: Serialize>(url: &str, req: &S) -> Result<ClientRequest,ClientError> {       
        Ok(ClientRequest {
            server_uri: url.parse().map_err(ClientError::Uri)?,
            json: Some(serde_json::to_string(req).map_err(ClientError::Json)?.into_bytes()),
            connect_timeout: None,
            request_timeout: None,
        })
    }
    pub fn get_from_url(url: &str) -> Result<ClientRequest,ClientError> {
        Ok(ClientRequest {
            server_uri: url.parse().map_err(ClientError::Uri)?,
            json: None,
            connect_timeout: None,
            request_timeout: None,
        })
    }
    pub fn with_connect_timeout(mut self, tm: std::time::Duration) -> ClientRequest {
        self.connect_timeout = Some(tm);
        self
    }
    pub fn with_request_timeout(mut self, tm: std::time::Duration) -> ClientRequest {
        self.request_timeout = Some(tm);
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
        let tm_uri = self.server_uri.to_string();
        let req = match self.json {
            Some(json) => Request::post(self.server_uri)
                .header(CONTENT_TYPE,mime::APPLICATION_JSON.as_ref())
                .header(CONTENT_LENGTH,json.len() as u64)
                .body(Body::from(json)),
            None => Request::get(self.server_uri).body(Body::empty()),
        }.map_err(ClientError::Request)?;


        let response = match self.request_timeout {
            None => client.request(req).await.map_err(ClientError::Hyper)?,
            Some(tm) => tokio::time::timeout(tm,client.request(req)).await
                .map_err(move |_| ClientError::Timeout(tm_uri))?
                .map_err(ClientError::Hyper)?,
        };
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
    request_timeout: Option<std::time::Duration>, 
}
impl<D: DeserializeOwned> Client<D> {
    pub fn new() -> Client<D> {
        Client {
            cli: client::Client::new(),
            _d: std::marker::PhantomData,
            request_timeout: None, 
        }
    }
    pub fn with_connect_timeout(self, timeout: std::time::Duration) -> Client<D> {
        Client {
            cli: client::Builder::default().build({
                let mut http_conn = HttpConnector::new();
                http_conn.set_connect_timeout(Some(timeout));
                http_conn
            }),
            request_timeout: None, 
            _d: std::marker::PhantomData,
        }
    }
    pub fn with_request_timeout(mut self, tm: std::time::Duration) -> Client<D> {
        self.request_timeout = Some(tm);
        self
    }
    pub async fn post_json<S: Serialize>(&self, uri: &str, req: &S) -> Result<D,ClientError> {
        let tm_uri = uri.to_string();
        let uri: Uri = uri.parse().map_err(ClientError::Uri)?;
        let json = serde_json::to_string(req).map_err(ClientError::Json)?.into_bytes();
        let req = Request::post(uri)
            .header(CONTENT_TYPE,mime::APPLICATION_JSON.as_ref())
            .header(CONTENT_LENGTH,json.len() as u64)
            .body(Body::from(json))
            .map_err(ClientError::Request)?;
        let response = match self.request_timeout {
            None => self.cli.request(req).await.map_err(ClientError::Hyper)?,
            Some(tm) => tokio::time::timeout(tm,self.cli.request(req)).await
                .map_err(move |_| ClientError::Timeout(tm_uri))?
                .map_err(ClientError::Hyper)?,
        };
        let buffer = body::to_bytes(response.into_body()).await.map_err(ClientError::Hyper)?;
        serde_json::from_slice(buffer.as_ref()).map_err(ClientError::Json)
    }
    pub async fn get(&self, uri: &str) -> Result<D,ClientError> {
        let tm_uri = uri.to_string();
        let uri: Uri = uri.parse().map_err(ClientError::Uri)?;
        let req = Request::get(uri)
            .body(Body::empty())
            .map_err(ClientError::Request)?;
        let response = match self.request_timeout {
            None => self.cli.request(req).await.map_err(ClientError::Hyper)?,
            Some(tm) => tokio::time::timeout(tm,self.cli.request(req)).await
                .map_err(move |_| ClientError::Timeout(tm_uri))?
                .map_err(ClientError::Hyper)?,
        };
        let buffer = body::to_bytes(response.into_body()).await.map_err(ClientError::Hyper)?;
        serde_json::from_slice(buffer.as_ref()).map_err(ClientError::Json)
    }
}


use hyper_tls::HttpsConnector;

pub struct TlsClient<D: DeserializeOwned> {
    cli: client::Client<HttpsConnector<client::HttpConnector>>,
    _d: std::marker::PhantomData<D>,
    request_timeout: Option<std::time::Duration>, 
}
impl<D: DeserializeOwned> TlsClient<D> {
    pub fn new() -> TlsClient<D> {
        TlsClient {
            cli: {
                let mut https = HttpsConnector::new();
                https.https_only(true);
                client::Client::builder().build::<_, hyper::Body>(https)
            },
            _d: std::marker::PhantomData,
            request_timeout: None, 
        }
    }
    pub fn with_request_timeout(mut self, tm: std::time::Duration) -> TlsClient<D> {
        self.request_timeout = Some(tm);
        self
    }
    pub async fn post_json<S: Serialize>(&self, uri: &str, req: &S) -> Result<D,ClientError> {
        let tm_uri = uri.to_string();
        let uri: Uri = uri.parse().map_err(ClientError::Uri)?;
        let json = serde_json::to_string(req).map_err(ClientError::Json)?.into_bytes();
        let req = Request::post(uri)
            .header(CONTENT_TYPE,mime::APPLICATION_JSON.as_ref())
            .header(CONTENT_LENGTH,json.len() as u64)
            .body(Body::from(json))
            .map_err(ClientError::Request)?;
        let response = match self.request_timeout {
            None => self.cli.request(req).await.map_err(ClientError::Hyper)?,
            Some(tm) => tokio::time::timeout(tm,self.cli.request(req)).await
                .map_err(move |_| ClientError::Timeout(tm_uri))?
                .map_err(ClientError::Hyper)?,
        };
        let buffer = body::to_bytes(response.into_body()).await.map_err(ClientError::Hyper)?;
        serde_json::from_slice(buffer.as_ref()).map_err(ClientError::Json)
    }
    pub async fn get(&self, uri: &str) -> Result<D,ClientError> {
        let tm_uri = uri.to_string();
        let uri: Uri = uri.parse().map_err(ClientError::Uri)?;
        let req = Request::get(uri)
            .body(Body::empty())
            .map_err(ClientError::Request)?;
        let response = match self.request_timeout {
            None => self.cli.request(req).await.map_err(ClientError::Hyper)?,
            Some(tm) => tokio::time::timeout(tm,self.cli.request(req)).await
                .map_err(move |_| ClientError::Timeout(tm_uri))?
                .map_err(ClientError::Hyper)?,
        };
        let buffer = body::to_bytes(response.into_body()).await.map_err(ClientError::Hyper)?;
        serde_json::from_slice(buffer.as_ref()).map_err(ClientError::Json)
    }
}



pub struct UriClient {
    cli: client::Client<client::HttpConnector>,
    uri: Uri,
}
impl UriClient {
    pub fn new(uri: &str) -> Result<UriClient,ClientError> {
        let uri: Uri = uri.parse().map_err(ClientError::Uri)?;
        Ok(UriClient {
            cli: client::Builder::default().build({
                let http_conn = HttpConnector::new();
                //http_conn.set_connect_timeout(Some(timeout));
                http_conn
            }),
            uri: uri,
        })
    }
    pub fn get(&self) -> Result<ReplyFuture,ClientError> {
        Ok(ReplyFuture{
            inner: ReplyFutureInner::Response(self.cli.request({
                Request::get(&self.uri)
                    .body(Body::empty())
                    .map_err(ClientError::Request)?
            }))
        })
    }
    pub fn post<S: Serialize>(&self, req: &S) -> Result<ReplyFuture,ClientError> {
        let json = serde_json::to_string(req).map_err(ClientError::Json)?.into_bytes();
        Ok(ReplyFuture{
            inner: ReplyFutureInner::Response(self.cli.request({
                Request::post(&self.uri)
                    .header(CONTENT_TYPE,mime::APPLICATION_JSON.as_ref())
                    .header(CONTENT_LENGTH,json.len() as u64)
                    .body(Body::from(json))
                    .map_err(ClientError::Request)?
            }))
        })
    }
}

use std::pin::Pin;
use futures::{
    Future,StreamExt,FutureExt,
    task::{Poll,Context},
    stream::Collect,
};
use hyper::body::Bytes;

enum ReplyFutureOutput {
    Response(Response<Body>),
    Collect(Vec<u8>),
}
enum ReplyFutureInner {
    Response(ResponseFuture),
    Collect(Collect<Body,Vec<Result<Bytes,hyper::Error>>>),
}
impl Future for ReplyFutureInner {
    type Output = Result<ReplyFutureOutput,ClientError>;
    
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.get_mut() {
            ReplyFutureInner::Response(fut) => std::pin::Pin::new(fut).poll(cx).map(|res| match res {
                Ok(rs) => Ok(ReplyFutureOutput::Response(rs)),
                Err(e) => Err(ClientError::Hyper(e)),
            }),
            ReplyFutureInner::Collect(fut) => std::pin::Pin::new(fut).poll(cx).map(|vres| -> Result<ReplyFutureOutput,ClientError> {
                let mut v = Vec::<u8>::new();
                for res in vres {
                    let bts = res.map_err(ClientError::Hyper)?;
                    v.extend(bts);                    
                }
                Ok(ReplyFutureOutput::Collect(v))
            }),
        }
    }
}

pub struct ReplyFuture {
    inner: ReplyFutureInner,
}
impl Future for ReplyFuture {
    type Output = Result<Vec<u8>,ClientError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mref = Pin::into_inner(self);
        loop {
            match mref.inner.poll_unpin(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Ready(Ok(out)) => match out {
                    ReplyFutureOutput::Response(rsp) => {
                        let body = rsp.into_body();
                        mref.inner = ReplyFutureInner::Collect(body.collect());
                    },
                    ReplyFutureOutput::Collect(v) => return Poll::Ready(Ok(v)),
                },
            }
        }
    }
}
