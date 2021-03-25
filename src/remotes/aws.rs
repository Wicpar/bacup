use s3::bucket::Bucket;
use s3::creds::Credentials;

use crate::config::AWSConfig;
use crate::remotes::uploader;

use std::io::prelude::*;
use std::io::Write;
use std::path::PathBuf;

use async_trait::async_trait;

use flate2::write::{GzEncoder, ZlibEncoder};
use flate2::Compression;

use chrono::{DateTime, Utc};

use std::fmt;

#[derive(Debug)]
pub enum Error {
    InvalidCredentials(s3::creds::AwsCredsError),
    InvalidBucket(s3::S3Error),
}

impl From<s3::creds::AwsCredsError> for Error {
    fn from(error: s3::creds::AwsCredsError) -> Self {
        Error::InvalidCredentials(error)
    }
}

impl From<s3::S3Error> for Error {
    fn from(error: s3::S3Error) -> Self {
        Error::InvalidBucket(error)
    }
}

impl std::error::Error for Error {}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::InvalidCredentials(error) => write!(f, "Invalid credentials: {}", error),
            Error::InvalidBucket(error) => write!(f, "Error creating bucket object: {}", error),
        }
    }
}

#[derive(Clone)]
pub struct AWSBucket {
    name: String,
    bucket: Bucket,
}

impl AWSBucket {
    pub fn new(config: AWSConfig, bucket_name: &str) -> Result<AWSBucket, Error> {
        let credentials = Credentials::new(
            Some(&config.access_key),
            Some(&config.secret_key),
            None,
            None,
            None,
        )?;
        let bucket = Bucket::new(bucket_name, config.region.parse().unwrap(), credentials)?;

        // Performa a listing request to check if the configuration is ok
        bucket.list_blocking(String::from("/"), Some(String::from("/")))?;
        return Ok(AWSBucket {
            name: String::from(bucket_name),
            bucket,
        });
    }
}

#[async_trait]
impl uploader::Uploader for AWSBucket {
    fn name(&self) -> String {
        return self.name.clone();
    }

    async fn upload_file(&self, path: PathBuf) -> Result<(), uploader::Error> {
        let mut content: Vec<u8> = vec![];
        let mut file = match std::fs::File::open(path.clone()) {
            Ok(file) => file,
            Err(error) => return Err(uploader::Error::LocalError(error)),
        };

        file.read_to_end(&mut content)?;
        let path = path.to_str().unwrap();
        self.bucket.put_object(path, &content).await?;
        Ok(())
    }

    async fn upload_file_compressed(&self, path: PathBuf) -> Result<(), uploader::Error> {
        let mut content: Vec<u8> = vec![];
        let mut file = match std::fs::File::open(path.clone()) {
            Ok(file) => file,
            Err(error) => return Err(uploader::Error::LocalError(error)),
        };

        file.read_to_end(&mut content)?;

        let mut e = ZlibEncoder::new(Vec::new(), Compression::default());
        e.write_all(&content)?;
        let compressed_bytes = match e.finish() {
            Ok(bytes) => bytes,
            Err(_) => return Err(uploader::Error::CompressionError),
        };

        let path = path.to_str().unwrap();
        self.bucket.put_object(path, &compressed_bytes).await?;
        Ok(())
    }

    async fn upload_folder(&self, path: PathBuf) -> Result<(), uploader::Error> {
        if !path.is_dir() {
            return Err(uploader::Error::NotADirectory);
        }

        let dirs = std::fs::read_dir(path)?
            .map(|res| res.map(|e| e.path()))
            .collect::<Result<Vec<_>, std::io::Error>>();

        let mut futures = vec![];

        for dir in dirs {
            for file in dir {
                futures.push(self.upload_file(file));
            }
        }

        futures::future::join_all(futures).await;
        Ok(())
    }

    async fn upload_folder_compressed(&self, path: PathBuf) -> Result<(), uploader::Error> {
        if !path.is_dir() {
            return Err(uploader::Error::NotADirectory);
        }

        let now: DateTime<Utc> = Utc::now();
        let archive_path = PathBuf::from(format!(
            "{}-{}.tar.zz",
            path.file_name().unwrap().to_str().unwrap(),
            now
        ));

        let archive = std::fs::File::create(&archive_path)?;
        let e = GzEncoder::new(archive, Compression::default());
        let mut tar = tar::Builder::new(e);
        tar.append_dir_all(".", path.clone())?;
        self.upload_file(path).await?;
        std::fs::remove_file(archive_path)?;
        Ok(())
    }
}
