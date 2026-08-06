use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

use http::{
    HeaderValue, Request, Response,
    header::{HeaderName, InvalidHeaderValue},
};
use tower::{Layer, Service};
use uuid::Uuid;

const REQUEST_ID_HEADER: HeaderName = HeaderName::from_static("x-request-id");

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct RequestId(Uuid);

impl RequestId {
    pub fn parse(value: &str) -> Result<Self, uuid::Error> {
        Uuid::parse_str(value).map(Self)
    }

    pub fn generate() -> Self {
        Self(Uuid::new_v4())
    }

    pub fn as_str(&self) -> String {
        self.0.to_string()
    }

    fn from_headers(headers: &http::HeaderMap) -> Self {
        headers
            .get(&REQUEST_ID_HEADER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| Self::parse(value).ok())
            .unwrap_or_else(Self::generate)
    }

    fn header_value(&self) -> Result<HeaderValue, InvalidHeaderValue> {
        HeaderValue::from_str(&self.as_str())
    }
}

impl std::fmt::Display for RequestId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Clone, Debug, Default)]
pub struct RequestIdLayer;

pub fn layer() -> RequestIdLayer {
    RequestIdLayer
}

impl<S> Layer<S> for RequestIdLayer {
    type Service = RequestIdService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        RequestIdService { inner }
    }
}

#[derive(Clone, Debug)]
pub struct RequestIdService<S> {
    inner: S,
}

impl<S, RequestBody, ResponseBody> Service<Request<RequestBody>> for RequestIdService<S>
where
    S: Service<Request<RequestBody>, Response = Response<ResponseBody>>,
    S::Future: Send + 'static,
{
    type Response = Response<ResponseBody>;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, context: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(context)
    }

    fn call(&mut self, mut request: Request<RequestBody>) -> Self::Future {
        let request_id = RequestId::from_headers(request.headers());
        request.extensions_mut().insert(request_id);
        let future = self.inner.call(request);

        Box::pin(async move {
            let mut response = future.await?;
            response.headers_mut().insert(
                REQUEST_ID_HEADER,
                request_id.header_value().expect("UUID is a header value"),
            );
            Ok(response)
        })
    }
}
