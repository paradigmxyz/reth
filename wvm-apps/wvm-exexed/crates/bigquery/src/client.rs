use indexmap::IndexMap;
use std::{any::Any, collections::HashMap};

use gcp_bigquery_client::{
    error::BQError,
    model::{
        table::Table, table_data_insert_all_request::TableDataInsertAllRequest,
        table_field_schema::TableFieldSchema, table_schema::TableSchema,
    },
    Client,
};

use phf::phf_ordered_map;
use polars::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use eyre::{Result, WrapErr};

/// Query client
/// Impl for this struct is further below
pub struct BigQueryClient {
    client: Client,
    project_id: String,
    dataset_id: String,
    // drop_tables: bool,
    // table_map: HashMap<String, IndexMap<String, String>>,
}

pub static COMMON_COLUMNS: phf::OrderedMap<&'static str, &'static str> = phf_ordered_map! {
    "indexed_id" => "string",  // will need to generate uuid in rust; postgres allows for autogenerate
    "block_number" => "int",
    "sealed_block_with_senders" => "string",
    "timestamp" => "int"
};

pub fn prepare_blockstate_table_config() -> HashMap<String, IndexMap<String, String>> {
    let mut table_column_definition: HashMap<String, IndexMap<String, String>> = HashMap::new();
    let merged_column_types: IndexMap<String, String> =
        COMMON_COLUMNS.into_iter().map(|it| (it.0.to_string(), it.1.to_string())).collect();

    table_column_definition.insert("state".to_string(), merged_column_types);
    table_column_definition
}

#[derive(Debug)]
pub enum GcpClientError {
    BigQueryError(BQError),
}

#[derive(Debug, Deserialize)]
pub struct BigQueryConfig {
    #[serde(rename = "dropTableBeforeSync")]
    // #[serde(skip_serializing_if = "Option::is_none")]
    pub drop_tables: bool,

    #[serde(rename = "projectId")]
    // #[serde(skip_serializing_if = "Option::is_none")]
    pub project_id: String,

    #[serde(rename = "datasetId")]
    // #[serde(skip_serializing_if = "Option::is_none")]
    pub dataset_id: String,

    #[serde(rename = "credentialsPath")]
    // #[serde(skip_serializing_if = "Option::is_none")]
    pub credentials_path: String,
}

pub async fn init_bigquery_db(
    bigquery_config: &BigQueryConfig,
) -> Result<BigQueryClient, GcpClientError> {
    let gcp_bigquery = BigQueryClient::new(bigquery_config).await?;

    // if bigquery_config.drop_tables {
    //     let res = gcp_bigquery.delete_tables().await;
    //     match res {
    //         Ok(..) => {}
    //         Err(err) => return Err(GcpClientError::BigQueryError(err)),
    //     }
    // }

    let res = gcp_bigquery.create_state_table().await;
    match res {
        Ok(..) => {}
        Err(err) => return Err(GcpClientError::BigQueryError(err)),
    }

    Ok(gcp_bigquery)
}

impl BigQueryClient {
    pub async fn new(
        bigquery_config: &BigQueryConfig,
        // indexer_contract_mappings: &[IndexerContractMapping],
    ) -> Result<Self, GcpClientError> {
        let client = gcp_bigquery_client::Client::from_service_account_key_file(
            bigquery_config.credentials_path.as_str(),
        )
        .await;

        // let table_map = load_table_configs(indexer_contract_mappings);

        match client {
            Err(error) => Err(GcpClientError::BigQueryError(error)),
            Ok(client) => Ok(BigQueryClient {
                client,
                // drop_tables: bigquery_config.drop_tables,
                project_id: bigquery_config.project_id.to_string(),
                dataset_id: bigquery_config.dataset_id.to_string(),
                //  table_map,
            }),
        }
    }

    ///
    /// Deletes tables from GCP bigquery, if they exist
    /// Tables are only deleted if the configuration has specified drop_table
    // pub async fn delete_tables(&self) -> Result<(), BQError> {
    //     if self.drop_tables {
    //         for table_name in self.table_map.keys() {
    //             let table_ref = self
    //                 .client
    //                 .table()
    //                 .get(
    //                     self.project_id.as_str(),
    //                     self.dataset_id.as_str(),
    //                     table_name.as_str(),
    //                     None,
    //                 )
    //                 .await;
    //
    //             if let Ok(table) = table_ref {
    //                 // Delete table, since it exists
    //                 let res = table.delete(&self.client).await;
    //                 match res {
    //                     Err(err) => {
    //                         return Err(err);
    //                     }
    //                     Ok(_) => println!("Removed table: {}", table_name),
    //                 }
    //             }
    //         }
    //     }
    //
    //     Ok(())
    // }

    ///
    /// Iterates through all defined tables, calls create_table on each table
    // pub async fn create_tables(&self) -> Result<(), BQError> {
    //     for (table_name, column_map) in self.table_map.iter() {
    //         let res = self.create_table(table_name, column_map).await;
    //         match res {
    //             Ok(..) => {}
    //             Err(err) => return Err(err),
    //         }
    //     }
    //     Ok(())
    // }

    pub async fn create_state_table(&self) -> Result<(), BQError> {
        for (table_name, column_map) in prepare_blockstate_table_config().iter() {
            let res = self.create_table(table_name, column_map).await;
            match res {
                Ok(..) => {}
                Err(err) => return Err(err),
            }
        }
        Ok(())
    }

    ///
    /// Create a single table in GCP bigquery, from configured datatypes
    ///
    /// # Arguments
    ///
    /// * `table_name` - name of table
    /// * `column_map` - map of column names to types
    pub async fn create_table(
        &self,
        table_name: &str,
        column_map: &IndexMap<String, String>,
    ) -> Result<(), BQError> {
        let dataset_ref =
            self.client.dataset().get(self.project_id.as_str(), self.dataset_id.as_str()).await;
        let table_ref = self
            .client
            .table()
            .get(self.project_id.as_str(), self.dataset_id.as_str(), table_name, None)
            .await;

        match table_ref {
            Ok(..) => {
                println!("Table {table_name} already exists, skip creation.");
                return Ok(());
            }
            Err(..) => {
                // Table does not exist (err), create
                // Transform config types to GCP api schema types
                let schema_types: Vec<TableFieldSchema> = column_map
                    .iter()
                    .map(|column| {
                        BigQueryClient::db_type_to_table_field_schema(
                            &column.1.to_string(),
                            &column.0.to_string(),
                        )
                    })
                    .collect();

                let dataset = &mut dataset_ref.as_ref().unwrap();
                let res = dataset
                    .create_table(
                        &self.client,
                        Table::new(
                            self.project_id.as_str(),
                            self.dataset_id.as_str(),
                            table_name,
                            TableSchema::new(schema_types),
                        ),
                    )
                    .await;

                match res {
                    Ok(_) => {
                        println!("Created table in gcp: {}", table_name);
                    }
                    Err(err) => return Err(err),
                }
            }
        }
        Ok(())
    }

    ///
    /// Constructs GCP bigquery rows for insertion into gcp tables
    /// The writer will write vector of all rows into dataset
    ///
    /// # Arguments
    ///
    /// * `df` - dataframe
    /// * `table_config` - column to type mapping for table being written to
    pub fn build_bigquery_rowmap_vector(
        &self,
        df: &DataFrame,
        table_config: &IndexMap<String, String>,
    ) -> Vec<HashMap<String, Value>> {
        let column_names = df.get_column_names();
        let mut vec_of_maps: Vec<HashMap<String, Value>> = Vec::with_capacity(df.height());

        for idx in 0..df.height() {
            let mut row_map: HashMap<String, Value> = HashMap::new();

            for name in &column_names {
                let series = df.column(name).expect("Column should exist");
                let value = series.get(idx).expect("Value should exist");

                //  Convert from AnyValue (polars) to Value (generic value wrapper)
                //  The converted-to type is already specified in the inbound configuration
                let col_type = table_config.get(&name.to_string()).expect("Column should exist");
                let transformed_value = match col_type.as_str() {
                    "int" => BigQueryClient::bigquery_anyvalue_numeric_type(&value),
                    "string" => Value::String(value.to_string()),
                    _ => Value::Null,
                };

                row_map.insert((*name).into(), transformed_value);
            }

            vec_of_maps.push(row_map);
        }

        vec_of_maps
    }

    ///
    /// Write vector of rowmaps into GCP.  Construct bulk insertion request and
    /// and insert into target remote table
    ///
    /// # Arguments
    ///
    /// * `table_name` - name of table being operated upon / written to
    /// * `vec_of_rowmaps` - vector of rowmap objects to facilitate write
    pub async fn write_rowmaps_to_gcp(
        &self,
        table_name: &str,
        vec_of_rowmaps: &Vec<HashMap<String, Value>>,
    ) {
        let mut insert_request = TableDataInsertAllRequest::new();
        for row_map in vec_of_rowmaps {
            let _ = insert_request.add_row(None, row_map.clone());
        }

        println!("row cnt gcp: {:?}", vec_of_rowmaps.len());
        println!("gcp insert request: {:?}", insert_request);

        let result = self
            .client
            .tabledata()
            .insert_all(
                self.project_id.as_str(),
                self.dataset_id.as_str(),
                table_name,
                insert_request,
            )
            .await;

        match result {
            Ok(response) => println!("Success response: {:?}", response),
            Err(response) => println!("Failed, reason: {:?}", response),
        }
    }

    ///
    ///  Maps converted column types and constructs bigquery column
    ///
    /// # Arguments
    ///
    /// * `db_type` - stored datatype configured for this column
    /// * `name` - name of column
    pub fn db_type_to_table_field_schema(db_type: &str, name: &str) -> TableFieldSchema {
        match db_type {
            "int" => TableFieldSchema::integer(name),
            "string" => TableFieldSchema::string(name),
            _ => panic!("Unsupported db type: {}", db_type),
        }
    }

    ///
    ///  Converts AnyValue types to numeric types used in GCP api
    ///
    /// # Arguments
    ///
    /// * `value` - value and type
    pub fn bigquery_anyvalue_numeric_type(value: &AnyValue) -> Value {
        match value {
            AnyValue::Int8(val) => Value::Number((*val).into()),
            AnyValue::Int16(val) => Value::Number((*val).into()),
            AnyValue::Int32(val) => Value::Number((*val).into()),
            AnyValue::Int64(val) => Value::Number((*val).into()),
            AnyValue::UInt8(val) => Value::Number((*val).into()),
            AnyValue::UInt16(val) => Value::Number((*val).into()),
            AnyValue::UInt32(val) => Value::Number((*val).into()),
            AnyValue::UInt64(val) => Value::Number((*val).into()),
            _ => Value::Null,
        }
    }

    pub async fn bq_insert_state(
        &self,
        table_name: &str,
        state: types::types::ExecutionTipState,
    ) -> eyre::Result<()> {
        // vec_of_rowmaps: &Vec<HashMap<String, Value>>,

        #[derive(Serialize)]
        struct StateRow {
            block_number: u64,
            sealed_block_with_senders: String,
        }

        let mut insert_request = TableDataInsertAllRequest::new();
        let json_sealed_block_with_senders =
            serde_json::to_string(&state.sealed_block_with_senders)?;

        insert_request.add_row(
            None,
            StateRow {
                block_number: state.block_number,
                sealed_block_with_senders: json_sealed_block_with_senders,
            },
        )?;

        println!("gcp insert request: {:?}", insert_request);

        let result = self
            .client
            .tabledata()
            .insert_all(
                self.project_id.as_str(),
                self.dataset_id.as_str(),
                table_name,
                insert_request,
            )
            .await;

        match result {
            Ok(response) => {
                println!("Success response: {:?}", response);
                Ok(())
            }
            Err(error) => {
                println!("Failed, reason: {:?}", error);
                Err(eyre::Report::new(error)).wrap_err("Failed to insert data into BigQuery")
            }
        }
    }
}
