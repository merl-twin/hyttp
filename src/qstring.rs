use url::{
    Url,
    form_urlencoded::Parse,
    percent_encoding::percent_decode,
};
use std::{
    str::Utf8Error,
    borrow::Cow,
    collections::BTreeMap,
    fmt::Debug,
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
    pub fn into_map<'t>(&self) -> QueryMap {
        let mut hash = BTreeMap::new();
        for (k,v) in self.parsed.query_pairs() {
            hash.entry(k.to_string()).or_insert(Value::None).push(v.to_string());            
        }
        hash
    }
}

pub type QueryMap = BTreeMap<String,Value<String>>;

#[derive(Debug)]
pub enum Value<T> {
    None,
    One(T),
    Vec(Vec<T>),
}
impl<T> Value<T> {
    pub fn push(&mut self, value: T) {
        let mut tmp = Value::None;
        std::mem::swap(&mut tmp,self);
        *self = match tmp {
            Value::None => Value::One(value),
            Value::One(v) => Value::Vec(vec![v,value]),
            Value::Vec(mut v) => {
                v.push(value);
                Value::Vec(v)
            },
        };
    }
    pub fn try_map<Q,F,E>(&self, func: F) -> Result<Value<Q>,E>
    where F: Fn(&T) -> Result<Q,E>,
          E: Debug
    {
        Ok(match self {
            Value::None => Value::None,
            Value::One(v) => Value::One(func(v)?),
            Value::Vec(vs) => Value::Vec({
                let mut nv = Vec::with_capacity(vs.len());
                for v in vs.iter() {
                    nv.push(func(v)?);
                }
                nv
            }),
        })
    }
}

/*
impl<'t,T,Q> TryInto<Value<Q>> for &'t Value<T>
where Q: TryFrom<&'t T>
{
    type Error = <Q as TryFrom<&'t T>>::Error;
    
    fn try_into(self) -> Result<Value<Q>,Self::Error>       
    {
        Ok(match self {
            Value::None => Value::None,
            Value::One(v) => Value::One(Q::try_from(v)?),
            Value::Vec(vs) => Value::Vec({
                let mut nv = Vec::with_capacity(vs.len());
                for v in vs.iter() {
                    nv.push(Q::try_from(v)?);
                }
                nv
            }),
        })
    }
}
*/

#[derive(Debug)]
pub enum UrlParseError {
    Url(url::ParseError),
    Utf8(Utf8Error),
}
