//! A simple static file service for AWS S3 using Axum.
//! 
//! This will retrieve the files from S3 and serve them as responses using a tower Service.
//! This is useful for serving static files from S3 in a serverless environment, when the
//! static files are independent of the application.
//! 
//! This provides local fallback for development (local axum invocation) as well.
//! 
//! # Basic Usage
//! 
//! ```rust
//! use axum::{Router, routing::get};
//! use axum_static_s3::S3OriginBuilder;
//! 
//! 
//! #[tokio::main]
//! async fn main() {
//!     // Build the S3 origin
//!     let s3_origin = S3OriginBuilder::new()
//!         .bucket("my-static-files-bucket")
//!         .prefix("static/")
//!         .prune_path(1)      // Remove the first request path component ()
//!         .max_size(1024 * 1024 * 12) // 12MB
//!         .build()
//!         .await
//!         .expect("Failed to build S3 origin");
//! 
//!     // Create the router with the S3 static file handler
//!     let app = Router::new()
//!         .nest_service("/static/", s3_origin);
//! 
//!     // Start the server
//!     let listener = tokio::net::TcpListener::bind("0.0.0.0:3000")
//!         .await
//!         .unwrap();
//!     axum::serve(listener, app).await.unwrap();
//! }
//! ```
//! 
//! # Features
//! 
//! - `trace`: Enable tracing of the S3 requests.
//! 
//! 
//! 
use std::sync::Arc;

use aws_config::SdkConfig as AwsSdkConfig;
use aws_sdk_s3::{
    Client as S3Client,
    error::SdkError,
    operation::get_object::{
        GetObjectError, 
        GetObjectOutput, 
        builders::GetObjectFluentBuilder
    },
};
use axum::response::IntoResponse;
use futures_core::stream::Stream;
use pin_project::pin_project;
use std::{
    convert::Infallible,
    future::Future,
    io::Error,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::io::{AsyncRead, ReadBuf};
use tower_service::Service;


#[cfg(feature = "trace")]
#[allow(unused_imports)]
use tracing::{info, error};
#[cfg(feature = "trace")]
#[allow(unused_imports)]
use tracing::Instrument;

#[allow(unused_macros)]
#[cfg(not(feature = "trace"))]
// Convert to a no-op macro
macro_rules! info {
    ($($arg:tt)*) => {
        ();
    };
}

#[derive(Clone)]
struct S3OriginInner {
    bucket: String,
    bucket_prefix: String,
    s3_client: Arc<S3Client>,
    prune_path: usize,
    max_size: Option<i64>,
}

#[derive(Clone)]
pub struct S3Origin {
    inner: Arc<S3OriginInner>,
}


pub struct S3OriginBuilder {
    bucket: Option<String>,
    bucket_prefix: Option<String>,
    s3_client: Option<S3Client>,
    aws_sdk_config: Option<AwsSdkConfig>,
    prune_path: usize,
    max_size: Option<i64>,
}


impl S3OriginBuilder {
    pub fn new() -> Self {
        Self {
            bucket: None,
            bucket_prefix: None,
            s3_client: None,
            aws_sdk_config: None,
            prune_path: 0,
            max_size: None,
        }
    }

    /// Set the bucket name.
    /// 
    /// This is required.
    /// 
    pub fn bucket(mut self, bucket: impl Into<String>) -> Self {
        self.bucket = Some(bucket.into());
        self
    }

    /// Set the bucket prefix.
    /// 
    /// This is optional, and defaults to an empty string.
    /// 
    pub fn prefix(mut self, prefix: impl Into<String>) -> Self {
        self.bucket_prefix = Some(prefix.into());
        self
    }

    /// Set the S3 client.
    /// 
    /// This is optional, and defaults to a new client created from the AWS SDK config.
    /// 
    pub fn client(mut self, client: S3Client) -> Self {
        self.s3_client = Some(client);
        self
    }

    /// Number of path components to remove from the request path.
    /// 
    /// This is useful for removing the bucket and prefix from the request path.
    /// 
    /// For example, if the request path is `/stage/my-app/static/deployment/index.html`,
    /// and the prune_path is 3, then the search key will be `{bucket}/{bucket_prefix/}deployment/index.html`.
    /// 
    pub fn prune_path(mut self, prune_path: usize) -> Self {
        self.prune_path = prune_path;
        self
    }

    /// Set the AWS SDK config.
    /// 
    /// This is optional, and defaults to a new client created from the AWS SDK config.
    /// If `client` is not provided, the AWS SDK config **must** be provided.
    /// 
    pub fn config(mut self, config: AwsSdkConfig) -> Self {
        self.aws_sdk_config = Some(config);
        self
    }

    /// Set the maximum size of the file to serve.
    /// 
    /// This is optional, and defaults to no maximum size.
    /// If the origin returns a file larger than the maximum size, an HTTP 413 (Payload Too Large) is returned.
    /// 
    pub fn max_size(mut self, max_size: i64) -> Self {
        self.max_size = Some(max_size);
        self
    }

    /// Build the S3 origin.
    /// 
    /// This will return an error a required parameter is not provided.
    /// 
    pub fn build(self) -> Result<S3Origin, &'static str> {
        let bucket = self.bucket.ok_or("bucket is required")?;
        let bucket_prefix = self.bucket_prefix.unwrap_or_default();
        
        let s3_client = if let Some(client) = self.s3_client {
            client
        } else if let Some(config) = self.aws_sdk_config {
            S3Client::new(&config)
        } else {
            return Err("either s3_client or aws_sdk_config must be provided");
        };

        Ok(S3Origin {
            inner: Arc::new(S3OriginInner {
                bucket,
                bucket_prefix,
                s3_client: Arc::new(s3_client),
                prune_path: self.prune_path,
                max_size: self.max_size,
            })
        })
    }
}
impl Default for S3OriginBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Takes a request and trims the paths and creates a new S3 key
fn request_to_key(bucket_prefix: &str, uri_path: &str, prune_path: usize) -> String {
    let request_path: String = match prune_path {
        0 => uri_path.to_string(),
        _ => uri_path.split('/').skip(prune_path).collect::<Vec<_>>().join("/"),
    };

    format!("{}{}", bucket_prefix, request_path.trim_start_matches('/'))
}


impl Service<axum::extract::Request> for S3Origin {
    type Error = Infallible;
    type Response = axum::response::Response<axum::body::Body>;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send + 'static >>;

    /// Always ready to serve; no backpressure.
    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    /// Serve the request.
    fn call(&mut self, req: axum::extract::Request) -> Self::Future {
        #[cfg(feature = "trace")]
        tracing::info!("S3Origin: Serving request");

        // Only GET requests are supported
        if req.method() != axum::http::Method::GET {
            #[cfg(feature = "trace")]
            tracing::info!("S3Origin: {} method not allowed", req.method());

            return Box::pin(async move {
                Ok(axum::response::Response::builder().status(axum::http::StatusCode::METHOD_NOT_ALLOWED).body(axum::body::Body::from("Method not allowed")).unwrap())
            });
        }

        let this = self.inner.clone();
        let path = req.uri().path();
        let path = path.strip_prefix("/").unwrap_or(path);

        let mut path = path.to_string();

        if this.prune_path > 0 {
            path = path.split('/').skip(this.prune_path).collect::<Vec<_>>().join("/");
        }

        let client = this.s3_client.clone();
        let key = request_to_key(&this.bucket_prefix, &path, this.prune_path);

        #[cfg(feature = "trace")]
        {
            let current_span = tracing::Span::current();
            current_span.record("s3_url", &format!("s3://{}/{}", this.bucket, key));
        }

        let get_s3_fut = async move {
            let builder = client.get_object()
                .bucket(&this.bucket)
                .key(&key);
            let builder = make_request_builder(&req, builder);

            let response;
            #[cfg(feature = "trace")]
            {
                response = builder.send()
                    .instrument(
                        tracing::info_span!("s3_get_object", bucket = %this.bucket, key = %key)
                    ).await;
            }
            #[cfg(not(feature = "trace"))]
            {
                response = builder.send().await;
            }
            
            let rv = wrap_create_response(response, this.max_size)
                .unwrap_or_else(|e| {
                    e.into_response()
            });

            Ok(rv)
        };

        Box::pin(get_s3_fut)
    }
}


fn make_request_builder(request: &axum::extract::Request, mut builder: GetObjectFluentBuilder) -> GetObjectFluentBuilder {
    // Check if there is a range header
    if let Some(range) = request.headers().get(axum::http::header::RANGE) {
        builder = builder.range(range.to_str().unwrap());
    }

    builder
}


fn wrap_create_response<E>(s3_response: Result<GetObjectOutput, SdkError<GetObjectError, E>>, max_size: Option<i64>) -> Result<axum::response::Response, S3Error> {
    #[cfg(feature = "trace")]
    {
        tracing::debug!("S3Origin: Wrapping response: {}",
            if s3_response.is_ok() { "OK".to_owned() } else { format!("Error: {}", s3_response.as_ref().err().unwrap().to_string()) }
        );
    }

    // Unwrap the response from S3, mapping to an S3Error if there is an error
    let s3_response = s3_response.map_err(S3Error::from)?;

    // Response was successful, so we can collect metadata
    let content_type = s3_response.content_type().map(|ct| ct.to_owned());
    let content_length = s3_response.content_length().map(|cl| cl.to_owned());

    if let Some(max_size) = max_size {
        if let Some(size) = content_length.as_ref() {
            if size > &max_size {
                return Err(S3Error::MaxSizeExceeded);
            }
        }
    }

    let body = TryStreamAdapater { stream: s3_response.body.into_async_read()};
    let body = axum::body::Body::from_stream(body);
    let mut response = axum::response::Response::builder()
        .status(200)
        .body(body)
        .unwrap(); // Safe to unwrap because we know the response is Ok and no headers are set

    // set Content-Type
    if let Some(content_type) = content_type {
        response.headers_mut().insert(
            axum::http::header::CONTENT_TYPE,
            content_type
                .parse()
                .map_err(|_| S3Error::InternalServerError)?
                );
    } else {
        response.headers_mut().insert(axum::http::header::CONTENT_TYPE, "application/octet-stream".parse().unwrap());  // UNWRAP: Safe value
    }
    // set Content-Length
    if let Some(content_length) = content_length {
        response.headers_mut().insert(axum::http::header::CONTENT_LENGTH, content_length.to_string().parse().unwrap());  // UNWRAP: Safe value
    }

    Ok(response)
}


impl<E> From<SdkError<GetObjectError, E>> for S3Error {
    fn from(error: SdkError<GetObjectError, E>) -> Self {
        match error {
            SdkError::ServiceError(error) => {
                if error.err().is_no_such_key() {
                    S3Error::NotFound
                } else {
                    S3Error::BadGateway
                }
            }
            _ => S3Error::InternalServerError,
        }
    }
}

impl axum::response::IntoResponse for S3Error {
    fn into_response(self) -> axum::response::Response {
        #[warn(unreachable_patterns)]
        match self {
            S3Error::NotFound => axum::response::Response::builder().status(axum::http::StatusCode::NOT_FOUND).body(axum::body::Body::from("Not found")).unwrap(),
            S3Error::BadGateway => axum::response::Response::builder().status(axum::http::StatusCode::BAD_GATEWAY).body(axum::body::Body::from("Bad gateway")).unwrap(),
            S3Error::InternalServerError => axum::response::Response::builder().status(axum::http::StatusCode::INTERNAL_SERVER_ERROR).body(axum::body::Body::from("Internal server error")).unwrap(),
            S3Error::MaxSizeExceeded => axum::response::Response::builder().status(axum::http::StatusCode::PAYLOAD_TOO_LARGE).body(axum::body::Body::from("Requested file size exceeds the maximum allowed size")).unwrap(),
        }
    }
}


#[pin_project]
struct TryStreamAdapater<T> {
    #[pin]
    stream: T,
}



impl<T: AsyncRead> Stream for TryStreamAdapater<T> {
    type Item = Result<Vec<u8>, Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut buf = [0; 1024];
        let mut read_buf = ReadBuf::new(&mut buf);

        let this = self.project();
        let stream = this.stream;
        
        match stream.poll_read(cx, &mut read_buf) {
            Poll::Ready(Ok(())) => {
                let n = read_buf.filled().len();
                if n > 0 {
                    Poll::Ready(Some(Ok(buf[..n].to_vec())))
                } else {
                    Poll::Ready(None)
                }
            }
            Poll::Ready(Err(e)) => Poll::Ready(Some(Err(e))),
            Poll::Pending => Poll::Pending,
        }
    }
}

enum S3Error {
    NotFound,
    BadGateway,
    InternalServerError,
    MaxSizeExceeded,
}


#[cfg(test)]
mod tests {
    use super::*;

    #[allow(dead_code)]
    fn assert_clone<T: Clone>(_: &T) { }
    #[allow(dead_code)]
    fn assert_send<T: Send>(_: &T) { }
    #[allow(dead_code)]
    fn assert_sync<T: Sync>(_: &T) { }
    #[allow(dead_code)]
    fn assert_service<T,R: Service<axum::extract::Request>>(_: T) { }

    #[test]
    fn can_route_to_s3_origin() {
        use axum::{Router, routing::get};
        let origin = S3OriginBuilder::new()
            .bucket("my-bucket")
            .prefix("my-prefix")
            .build()
            .unwrap();
        
        #[allow(dead_code, unused_must_use)]
        Router::<()>::new().nest_service("/static", origin);
    }

    #[test]
    fn test_nest_route_route() {
        use axum::{Router, routing::get};
        let subroute: Router<()> = Router::new().route("/", get(|| async { "Hello, world!" }));
        let app = Router::new().nest("/foo", subroute);
    }

}