use crate::err::AppError;
use crate::models::{
    ManageEventRequest, RegisterEventRequest, RegisterEventResponse, StartEventResponse,
};
use uuid::Uuid;

pub struct InsightsClient {
    client: reqwest::Client,
    address: String,
}

pub fn new(address: String) -> InsightsClient {
    InsightsClient {
        client: reqwest::Client::new(),
        address,
    }
}

impl InsightsClient {
    pub async fn register_event(&self, req: RegisterEventRequest) -> Result<String, AppError> {
        let response = self
            .client
            .post(format!("{}/api/v1/event/register", self.address))
            .json(&req)
            .send()
            .await?;

        match response.status() {
            reqwest::StatusCode::OK => Ok(response
                .json::<RegisterEventResponse>()
                .await
                .map_err(|_| "insights: cannot parse register response")?
                .insights_link),
            status => Err(format!("insights: register failed with {}", status).into()),
        }
    }

    pub async fn start_event(&self, event_id: Uuid) -> Result<String, AppError> {
        let response = self
            .client
            .post(format!("{}/api/v1/event/start", self.address))
            .json(&ManageEventRequest { event_id })
            .send()
            .await?;

        match response.status() {
            reqwest::StatusCode::OK => Ok(response
                .json::<StartEventResponse>()
                .await
                .map_err(|_| "insights: cannot parse start response")?
                .summary_link),
            status => Err(format!("insights: start failed with {}", status).into()),
        }
    }

    pub async fn finish_event(&self, event_id: Uuid) -> Result<(), AppError> {
        let response = self
            .client
            .post(format!("{}/api/v1/event/finish", self.address))
            .json(&ManageEventRequest { event_id })
            .send()
            .await?;

        match response.status() {
            reqwest::StatusCode::OK => Ok(()),
            status => Err(format!("insights: finish failed with {}", status).into()),
        }
    }
}
