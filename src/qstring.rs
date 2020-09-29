use url::{
    Url,
    form_urlencoded::Parse,
    percent_encoding::percent_decode,
};
use std::{
    str::Utf8Error,
    borrow::Cow,
};
use hyper::Uri;

#[derive(Debug,Clone)]
pub struct QueryString {
    parsed: Url,
}
pub struct QueryStringIterator<'t,I>
    where I: Iterator<Item = (Cow<'t,str>,Cow<'t,str>)>
{
    iter: I,
    key: String,
}
impl<'t,I> Iterator for QueryStringIterator<'t,I>
    where I: Iterator<Item = (Cow<'t,str>,Cow<'t,str>)>
{
    type Item = Cow<'t,str>;
    
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            return match self.iter.next() {
                None => None,
                Some((k,v)) => if k == self.key {
                    Some(v)
                } else {
                    continue
                }
            };
        }
    }
}
impl  QueryString {
    pub fn from_uri(uri: &Uri) -> Result<QueryString,UrlParseError> {
        let decoded = percent_decode(uri.as_ref().as_bytes()).decode_utf8().map_err(UrlParseError::Utf8)?;
        let parsed_data = std::rc::Rc::new(format!("http://localhost{}",decoded));           
        let parsed = Url::parse(&parsed_data).map_err(UrlParseError::Url)?;
        Ok(QueryString{
            parsed: parsed,
        })
    }
    pub fn iter(&self) -> Parse {
        self.parsed.query_pairs()
    }
    pub fn key<'t>(&'t self, key: &str) -> QueryStringIterator<'t,Parse<'t>> {
        QueryStringIterator {
            iter: self.parsed.query_pairs().into_iter(),
            key: key.to_string(),
        }
    }
}

#[derive(Debug)]
pub enum UrlParseError {
    Url(url::ParseError),
    Utf8(Utf8Error),
}
