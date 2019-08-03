#[macro_use]
extern crate log;
extern crate serde;
#[macro_use]
extern crate serde_derive;
extern crate serde_json;
extern crate hyper;
extern crate futures;
extern crate tokio_core;

mod server;
mod jresponse;

pub use jresponse::{
    JsonResponse,
};

pub use server::{
    FrontendError,
    FrontendBuilder,
    ReactorControl,
    InitDestructor,
    ClientInfo,
    RequestDispatcher,
    ReplySender,
    HtmlSender,
    DispatchResult,
};


#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {

    }
}
