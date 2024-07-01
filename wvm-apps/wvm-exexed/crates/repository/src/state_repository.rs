use bigquery::client::BigQueryClient;

use types;

pub struct StateRepository {
    pub bq_client: BigQueryClient,
}

//noinspection ALL
impl StateRepository {
    pub fn new(bq_client: BigQueryClient) -> StateRepository {
        StateRepository { bq_client }
    }

    pub async fn save(&self, state: types::types::ExecutionTipState) -> eyre::Result<()> {
        self.bq_client.bq_insert_state("state", state).await
    }
}
