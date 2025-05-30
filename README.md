# Rust Axum S3 Static

Easily serve S3 content from an Axum [Route](https://docs.rs/axum/latest/axum/routing/struct.Route.html).

```rust
use axum::{Router, routing::get};
use axum_static_s3::S3OriginBuilder;

let s3_origin = S3OriginBuilder::new()
    .bucket("my-bucket")
    .prefix("deploy/")
    .max_size(1024 * 1024 * 12) // 12 MiB
    .build()

let Router = Router::new()
    .nest_service("/static", s3_origin)
```

## Description

In modern webapp development, the back-end may be hosted on a local workstation during development, and a serverless compute environment during deployment.  This crate makes it easy to serve S3 resources as a path in an Axum router.

## Features

- Serves static files from AWS S3
- Compatible with API Gateway -> Lambda back-end, serving front-end resources from S3
    - Can specify response size limits for proper Payload Too Large responses if origin exceeds serverless compute response size
- Built with Axum web framework
- Efficient file handling (streams body)
- Configurable through environment variables

## License

This project is licensed under the MIT License.

## Contributing

Contributions are welcome! Please feel free to submit a Pull Request or contact author directly.

## Acknowledgments

- [Axum](https://github.com/tokio-rs/axum) - Web framework
- [AWS SDK for Rust](https://github.com/awslabs/aws-sdk-rust) - AWS SDK
