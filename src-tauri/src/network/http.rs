use std::time::Duration;

use reqwest::RequestBuilder;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use url::Url;

use crate::error::{AppError, AppResult};

#[derive(Clone)]
pub struct HttpClient {
    client: reqwest::Client,
}

#[derive(Debug, Deserialize)]
struct ApiEnvelope<T> {
    code: i32,
    data: T,
    message: String,
}

#[derive(Debug, Deserialize)]
struct ApiOptionalEnvelope<T> {
    code: i32,
    _data: Option<T>,
    message: String,
}

impl HttpClient {
    pub fn new() -> AppResult<Self> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()?;

        Ok(Self { client })
    }

    pub async fn get<T>(&self, base_url: &str, path: &str, access_token: Option<&str>) -> AppResult<T>
    where
        T: DeserializeOwned,
    {
        let request = self.authorized(
            self.client.get(Self::endpoint(base_url, path)?),
            access_token,
        );
        self.send_data(request).await
    }

    pub async fn post<Req, Res>(
        &self,
        base_url: &str,
        path: &str,
        body: &Req,
        access_token: Option<&str>,
    ) -> AppResult<Res>
    where
        Req: Serialize + ?Sized,
        Res: DeserializeOwned,
    {
        let request = self
            .authorized(
                self.client.post(Self::endpoint(base_url, path)?),
                access_token,
            )
            .json(body);

        self.send_data(request).await
    }

    pub async fn post_empty<Req>(
        &self,
        base_url: &str,
        path: &str,
        body: &Req,
        access_token: Option<&str>,
    ) -> AppResult<()>
    where
        Req: Serialize + ?Sized,
    {
        let request = self
            .authorized(
                self.client.post(Self::endpoint(base_url, path)?),
                access_token,
            )
            .json(body);

        let (_, payload) = self.send_raw(request).await?;
        let envelope: ApiOptionalEnvelope<Value> = serde_json::from_str(&payload)?;

        if envelope.code != 0 {
            return Err(AppError::message(envelope.message));
        }

        Ok(())
    }

    fn authorized(&self, request: RequestBuilder, access_token: Option<&str>) -> RequestBuilder {
        match access_token {
            Some(token) => request.bearer_auth(token),
            None => request,
        }
    }

    fn endpoint(base_url: &str, path: &str) -> AppResult<String> {
        let base = Url::parse(base_url)?;
        Ok(base.join(path.trim_start_matches('/'))?.to_string())
    }

    async fn send_data<T>(&self, request: RequestBuilder) -> AppResult<T>
    where
        T: DeserializeOwned,
    {
        let (status, payload) = self.send_raw(request).await?;
        let envelope: ApiEnvelope<T> = serde_json::from_str(&payload)?;

        if envelope.code != 0 {
            return Err(AppError::message(envelope.message));
        }

        if !status.is_success() {
            return Err(AppError::message(format!("request failed with status {status}")));
        }

        Ok(envelope.data)
    }

    async fn send_raw(&self, request: RequestBuilder) -> AppResult<(reqwest::StatusCode, String)> {
        let response = request.send().await?;
        let status = response.status();
        let payload = response.text().await?;
        Ok((status, payload))
    }
}
