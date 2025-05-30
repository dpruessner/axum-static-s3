use std::sync::Arc;

use aws_sdk_s3::Client as S3Client;
use aws_config::SdkConfig as AwsSdkConfig;

use crate::S3Origin;

use super::S3OriginInner;


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