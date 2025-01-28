use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::result::Result::Ok;
use thiserror::Error;
use reth_storage_errors::db::DatabaseError;
use std::time::{SystemTime, UNIX_EPOCH};
use rand::{thread_rng, Rng};

const OP_PUT: u8 = 1;
const OP_GET: u8 = 2;
const OP_DELETE: u8 = 3;
const OP_WRITE: u8 = 4;

const OP_FIRST: u8 = 5;
const OP_SEEK_EXACT: u8 = 6;
const OP_SEEK: u8 = 7;
const OP_NEXT: u8 = 8;
const OP_PREV: u8 = 9;
const OP_LAST: u8 = 10;
const OP_CURRENT: u8 = 11;

const OP_UPSERT: u8 = 12;
const OP_INSERT: u8 = 13;
const OP_APPEND: u8 = 14;
const OP_DELETE_CURRENT: u8 = 15;

const OP_NEXT_DUP: u8 = 16;
const OP_NEXT_NO_DUP: u8 = 17;
const OP_NEXT_DUP_VAL: u8 = 18;
const OP_SEEK_BY_KEY_SUBKEY: u8 = 19;

const OP_DELETE_CURRENT_DUPLICATE: u8 = 20;
const OP_APPEND_DUP: u8 = 21;

const STATUS_SUCCESS: u8 = 1;
const STATUS_ERROR: u8 = 0;

const SOCKET_PATH: &str = "/tmp/scalerize";

/// Represents errors that can occur while interacting with the Scalerize client.
///
/// This enum is used to categorize different types of errors that may arise during
/// operations such as I/O errors, operation failures, and invalid responses from the server.
#[derive(Error, Debug)]
pub enum ClientError {
	/// An I/O error occurred.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

	/// The requested operation failed with a specific message.
    #[error("Operation failed: {0}")]
    OperationFailed(String),

    /// The request made is invalid.
    #[error("Operation failed: {0}")]
    InvalidRequest(String),

	/// The response received from the server was invalid.
    #[error("Invalid response from server: {0}")]
    InvalidResponse(String),
}

impl From<ClientError> for DatabaseError {
    fn from(error: ClientError) -> Self {
        match error {
            ClientError::Io(err) => DatabaseError::Other(format!("IO error: {}", err)),
            ClientError::InvalidResponse(msg) => DatabaseError::Other(format!("Invalid response: {}", msg)),
            ClientError::InvalidRequest(msg) => DatabaseError::Other(format!("Invalid request: {}", msg)),
			ClientError::OperationFailed(msg) => DatabaseError::Other(format!("Operation failed: {}", msg)),
        }
    }
}


pub struct ScalerizeClient {
    stream: UnixStream,
}

impl ScalerizeClient {
    pub fn connect() -> Result<Self, ClientError> {
        let stream = UnixStream::connect(SOCKET_PATH)?;
        Ok(Self { stream })
    }

    fn log_response(response: &[u8]) {
        if response.is_empty() {
            println!("Empty response received");
            return;
        }

        let status = response[0];
        let data = &response[1..];
        
        println!("Server Response Status: {}", status);
        println!("Raw Response Data: {:?}", data);
        if let Ok(text) = String::from_utf8(data.to_vec()) {
            println!("Response as text: {}", text);
        }
    }

    fn read_full_response(&mut self) -> Result<Vec<u8>, ClientError> {
        let mut response = vec![0u8; 4096];
        let n = self.stream.read(&mut response)?;
        response.truncate(n);
        
        if response.is_empty() {
            return Err(ClientError::InvalidResponse("Empty response from server".to_string()));
        }
        
        Self::log_response(&response);
        Ok(response)
    }

	pub fn get(&mut self, table_code: u8, key: &[u8]) -> Result<Vec<u8>, ClientError> {
        println!("KEY FOR GET: {:?}", key);
        let mut request = vec![OP_GET];
        request.extend_from_slice(&table_code.to_be_bytes());
        request.extend_from_slice(key);
        
        println!("GET REQUEST: {:?}", request);
        self.stream.write_all(&request)?;
        self.stream.flush()?;

        let response = self.read_full_response()?;
        println!("RESPONSE FOR GET: {:?}", response);
        // Ok(response)
        let status = response[0];
        let data = response[1..].to_vec();

        match status {
            STATUS_SUCCESS => Ok(data),
            STATUS_ERROR => Err(ClientError::OperationFailed(String::from_utf8_lossy(&data).into_owned())),
            _ => Err(ClientError::OperationFailed(format!("Error: {:?}", data)))
        }
    }

    // no need to send rlp encoded for dupsorted even when using this method
    // just send the value at that subkey
    pub fn put(&mut self, table_code: u8, key: &[u8], subkey: Option<&[u8]>, value: &[u8]) -> Result<(), ClientError> {
        let mut request = vec![OP_PUT, table_code];
        
        request.extend_from_slice(key);
        if let Some(subkey) = subkey {
            request.extend_from_slice(subkey);
        }           request.extend_from_slice(value);
        
        println!("PUT REQUEST: {:?}", request);
        self.stream.write_all(&request)?;
        self.stream.flush()?;
    
        let response = self.read_full_response()?;
        println!("RESPONSE FOR PUT: {:?}", response);

        let status = response[0];
        let data = response[1..].to_vec();

        match status {
            STATUS_SUCCESS => Ok(()),
            STATUS_ERROR => Err(ClientError::OperationFailed(String::from_utf8_lossy(&data).into_owned())),
            _ => Err(ClientError::OperationFailed(format!("Error: {:?}", data)))
        }
    }

    // pub fn delete(&mut self, table_code: u8, key: &[u8], value: &[u8]) -> Result<Vec<u8>, ClientError> {
    //     let mut request = vec![OP_DELETE, table_code];
    //     request.extend_from_slice(key);
    //     request.extend_from_slice(value);
        
    //     println!("DELETE REQUEST: {:?}", request);
    //     self.stream.write_all(&request)?;
    //     self.stream.flush()?;

    //     let response = self.read_full_response()?;
    //     println!("RESPONSE FOR DELETE: {:?}", response);
    //     let status = response[0];
    //     let data = response[1..].to_vec();

    //     match status {
    //         STATUS_SUCCESS => Ok(data),
    //         STATUS_ERROR => Err(ClientError::OperationFailed(String::from_utf8_lossy(&data).into_owned())),
    //         _ => Err(ClientError::OperationFailed(format!("Error: {:?}", data)))
    //     }
    // }

    pub fn delete(&mut self, table_code: u8, key: &[u8], subkey: Option<&[u8]>) -> Result<(), ClientError> {
        let mut request = vec![OP_DELETE, table_code];
        request.extend_from_slice(key);
        if let Some(subkey) = subkey {
            request.extend_from_slice(subkey);
        }        
        println!("DELETE REQUEST: {:?}", request);
        self.stream.write_all(&request)?;
        self.stream.flush()?;

        let response = self.read_full_response()?;
        println!("RESPONSE FOR DELETE: {:?}", response);
        let status = response[0];
        let data = response[1..].to_vec();

        match status {
            STATUS_SUCCESS => Ok(()),
            STATUS_ERROR => Err(ClientError::OperationFailed(String::from_utf8_lossy(&data).into_owned())),
            _ => Err(ClientError::OperationFailed(format!("Error: {:?}", data)))
        }
    }

    pub fn write(&mut self) -> Result<Vec<u8>, ClientError> {
        let store_number: u8 = 0;
        let mut request = vec![OP_WRITE];
        request.extend_from_slice(&store_number.to_be_bytes());
        
        println!("WRITE REQUEST: {:?}", request);
        self.stream.write_all(&request)?;
        self.stream.flush()?;

        let response = self.read_full_response()?;
        println!("RESPONSE FOR WRITE: {:?}", response);
        let status = response[0];
        let data = response[1..].to_vec();

        match status {
            STATUS_SUCCESS => Ok(data),
            STATUS_ERROR => Err(ClientError::OperationFailed(String::from_utf8_lossy(&data).into_owned())),
            _ => Err(ClientError::OperationFailed(format!("Error: {:?}", data)))
        }
    }

    pub fn first(&mut self, table_code: u8, cursor_id: Vec<u8>, key_len: u32) -> Result<(Vec<u8>, Vec<u8>), ClientError> {
        let mut request = vec![OP_FIRST];
        request.extend_from_slice(&table_code.to_be_bytes());
        request.extend_from_slice(&cursor_id);
        
        println!("FIRST REQUEST: {:?}", request);
        self.stream.write_all(&request)?;
        self.stream.flush()?;

        let response = self.read_full_response()?;
        let status = response[0];
        let data = &response[1..];

        match status {
            STATUS_SUCCESS => self.parse_key_value_response(data, key_len),
            STATUS_ERROR => Err(ClientError::OperationFailed(String::from_utf8_lossy(data).into_owned())),
            _ => Err(ClientError::InvalidResponse(format!("Unexpected status: {}", status)))
        }
    }

    pub fn seek_exact(&mut self, table_code: u8, cursor_id: Vec<u8>, key: &[u8]) -> Result<Vec<u8>, ClientError> {
        let mut request = vec![OP_SEEK_EXACT];
        request.extend_from_slice(&table_code.to_be_bytes());
        request.extend_from_slice(&cursor_id);
        request.extend_from_slice(key);
        
        println!("SEEK_EXACT REQUEST: {:?}", request);
        self.stream.write_all(&request)?;
        self.stream.flush()?;

        let response = self.read_full_response()?;
        let status = response[0];
        let data = &response[1..];

        match status {
            STATUS_SUCCESS => Ok(data.to_vec()),
            STATUS_ERROR => Err(ClientError::OperationFailed(String::from_utf8_lossy(data).into_owned())),
            _ => Err(ClientError::InvalidResponse(format!("Unexpected status: {}", status)))
        }
    }

    pub fn seek(&mut self, table_code: u8, cursor_id: Vec<u8>, key: &[u8]) -> Result<(Vec<u8>, Vec<u8>), ClientError> {
        let mut request = vec![OP_SEEK];
        request.extend_from_slice(&table_code.to_be_bytes());
        request.extend_from_slice(&cursor_id);
        request.extend_from_slice(key);
        
        println!("SEEK REQUEST: {:?}", request);
        self.stream.write_all(&request)?;
        self.stream.flush()?;

        let response = self.read_full_response()?;
        let status = response[0];
        let data = &response[1..];

        match status {
            STATUS_SUCCESS => self.parse_key_value_response(data, key.len().try_into().unwrap()),
            STATUS_ERROR => Err(ClientError::OperationFailed(String::from_utf8_lossy(data).into_owned())),
            _ => Err(ClientError::InvalidResponse(format!("Unexpected status: {}", status)))
        }
    }

    pub fn next(&mut self, table_code: u8, cursor_id: Vec<u8>, key_len: u32) -> Result<(Vec<u8>, Vec<u8>), ClientError> {
        let mut request = vec![OP_NEXT];
        request.extend_from_slice(&table_code.to_be_bytes());
        request.extend_from_slice(&cursor_id);
        
        println!("NEXT REQUEST: {:?}", request);
        self.stream.write_all(&request)?;
        self.stream.flush()?;

        let response = self.read_full_response()?;
        let status = response[0];
        let data = &response[1..];

        match status {
            STATUS_SUCCESS => self.parse_key_value_response(data, key_len),
            STATUS_ERROR => Err(ClientError::OperationFailed(String::from_utf8_lossy(data).into_owned())),
            _ => Err(ClientError::InvalidResponse(format!("Unexpected status: {}", status)))
        }
    }

    pub fn prev(&mut self, table_code: u8, cursor_id: Vec<u8>, key_len: u32) -> Result<(Vec<u8>, Vec<u8>), ClientError> {
        let mut request = vec![OP_PREV];
        request.extend_from_slice(&table_code.to_be_bytes());
        request.extend_from_slice(&cursor_id);
        
        println!("PREV REQUEST: {:?}", request);
        self.stream.write_all(&request)?;
        self.stream.flush()?;

        let response = self.read_full_response()?;
        let status = response[0];
        let data = &response[1..];

        match status {
            STATUS_SUCCESS => self.parse_key_value_response(data, key_len),
            STATUS_ERROR => Err(ClientError::OperationFailed(String::from_utf8_lossy(data).into_owned())),
            _ => Err(ClientError::InvalidResponse(format!("Unexpected status: {}", status)))
        }
    }

    pub fn last(&mut self, table_code: u8, cursor_id: Vec<u8>, key_len: u32) -> Result<(Vec<u8>, Vec<u8>), ClientError> {
        let mut request = vec![OP_LAST];
        request.extend_from_slice(&table_code.to_be_bytes());
        request.extend_from_slice(&cursor_id);
        
        println!("LAST REQUEST: {:?}", request);
        self.stream.write_all(&request)?;
        self.stream.flush()?;

        let response = self.read_full_response()?;
        let status = response[0];
        let data = &response[1..];

        match status {
            STATUS_SUCCESS => self.parse_key_value_response(data, key_len),
            STATUS_ERROR => Err(ClientError::OperationFailed(String::from_utf8_lossy(data).into_owned())),
            _ => Err(ClientError::InvalidResponse(format!("Unexpected status: {}", status)))
        }
    }

    pub fn current(&mut self, table_code: u8, cursor_id: Vec<u8>, key_len: u32) -> Result<(Vec<u8>, Vec<u8>), ClientError> {
        let mut request = vec![OP_CURRENT];
        request.extend_from_slice(&table_code.to_be_bytes());
        request.extend_from_slice(&cursor_id);
        
        println!("PREV REQUEST: {:?}", request);
        self.stream.write_all(&request)?;
        self.stream.flush()?;

        let response = self.read_full_response()?;
        let status = response[0];
        let data = &response[1..];

        match status {
            STATUS_SUCCESS => self.parse_key_value_response(data, key_len),
            STATUS_ERROR => Err(ClientError::OperationFailed(String::from_utf8_lossy(data).into_owned())),
            _ => Err(ClientError::InvalidResponse(format!("Unexpected status: {}", status)))
        }
    }

    pub fn upsert(&mut self, table_code: u8, cursor_id: Vec<u8>, key: &[u8], subkey: Option<&[u8]>, value: &[u8]) -> Result<(), ClientError> {
        let mut request = vec![OP_UPSERT, table_code];
        request.extend_from_slice(&cursor_id);
        
        request.extend_from_slice(key);
        if let Some(subkey) = subkey {
            request.extend_from_slice(subkey);
        }           
        
        request.extend_from_slice(value);
        
        println!("UPSERT REQUEST: {:?}", request);
        self.stream.write_all(&request)?;
        self.stream.flush()?;
    
        let response = self.read_full_response()?;
        println!("RESPONSE FOR UPSERT: {:?}", response);

        let status = response[0];
        let data = response[1..].to_vec();

        match status {
            STATUS_SUCCESS => Ok(()),
            STATUS_ERROR => Err(ClientError::OperationFailed(String::from_utf8_lossy(&data).into_owned())),
            _ => Err(ClientError::OperationFailed(format!("Error: {:?}", data)))
        }
    }

    pub fn insert(&mut self, table_code: u8, cursor_id: Vec<u8>, key: &[u8], subkey: Option<&[u8]>, value: &[u8]) -> Result<(), ClientError> {
        let mut request = vec![OP_INSERT, table_code];
        request.extend_from_slice(&cursor_id);
        
        request.extend_from_slice(key);
        if let Some(subkey) = subkey {
            request.extend_from_slice(subkey);
        }           request.extend_from_slice(value);
        
        println!("INSERT REQUEST: {:?}", request);
        self.stream.write_all(&request)?;
        self.stream.flush()?;
    
        let response = self.read_full_response()?;
        println!("RESPONSE FOR INSERT: {:?}", response);

        let status = response[0];
        let data = response[1..].to_vec();

        match status {
            STATUS_SUCCESS => Ok(()),
            STATUS_ERROR => Err(ClientError::OperationFailed(String::from_utf8_lossy(&data).into_owned())),
            _ => Err(ClientError::OperationFailed(format!("Error: {:?}", data)))
        }
    }

    pub fn append(&mut self, table_code: u8, cursor_id: Vec<u8>, key: &[u8], subkey: Option<&[u8]>, value: &[u8]) -> Result<(), ClientError> {
        let mut request = vec![OP_APPEND, table_code];
        request.extend_from_slice(&cursor_id);
        
        request.extend_from_slice(key);
        if let Some(subkey) = subkey {
            request.extend_from_slice(subkey);
        }           request.extend_from_slice(value);
        
        println!("APPEND REQUEST: {:?}", request);
        self.stream.write_all(&request)?;
        self.stream.flush()?;
    
        let response = self.read_full_response()?;
        println!("RESPONSE FOR APPEND: {:?}", response);

        let status = response[0];
        let data = response[1..].to_vec();

        match status {
            STATUS_SUCCESS => Ok(()),
            STATUS_ERROR => Err(ClientError::OperationFailed(String::from_utf8_lossy(&data).into_owned())),
            _ => Err(ClientError::OperationFailed(format!("Error: {:?}", data)))
        }
    }

    pub fn delete_current(&mut self, table_code: u8, cursor_id: Vec<u8>) -> Result<(), ClientError> {
        let mut request = vec![OP_DELETE_CURRENT];
        request.extend_from_slice(&table_code.to_be_bytes());
        request.extend_from_slice(&cursor_id);
        
        println!("DELETE CURRENT REQUEST: {:?}", request);
        self.stream.write_all(&request)?;
        self.stream.flush()?;

        let response = self.read_full_response()?;
        let status = response[0];
        let data = &response[1..];

        match status {
            STATUS_SUCCESS => Ok(()),
            STATUS_ERROR => Err(ClientError::OperationFailed(String::from_utf8_lossy(data).into_owned())),
            _ => Err(ClientError::InvalidResponse(format!("Unexpected status: {}", status)))
        }
    }

    pub fn next_dup(&mut self, table_code: u8, cursor_id: Vec<u8>, key_len: u32) -> Result<Option<(Vec<u8>, Vec<u8>)>, ClientError> {
        let mut request = vec![OP_NEXT_DUP];
        request.extend_from_slice(&table_code.to_be_bytes());
        request.extend_from_slice(&cursor_id);
        
        println!("NEXT DUP REQUEST: {:?}", request);
        self.stream.write_all(&request)?;
        self.stream.flush()?;

        let response = self.read_full_response()?;
        let status = response[0];
        let data = &response[1..];

        if data.is_empty() {
            return Ok(None);
        }
        match status {
            STATUS_SUCCESS => self.parse_key_value_response(data, key_len).map(Some),
            STATUS_ERROR => Err(ClientError::OperationFailed(String::from_utf8_lossy(data).into_owned())),
            _ => Err(ClientError::InvalidResponse(format!("Unexpected status: {}", status)))
        }
    }

    pub fn next_no_dup(&mut self, table_code: u8, cursor_id: Vec<u8>, key_len: u32) -> Result<Option<(Vec<u8>, Vec<u8>)>, ClientError> {
        let mut request = vec![OP_NEXT_NO_DUP];
        request.extend_from_slice(&table_code.to_be_bytes());
        request.extend_from_slice(&cursor_id);
        
        println!("NEXT DUP REQUEST: {:?}", request);
        self.stream.write_all(&request)?;
        self.stream.flush()?;

        let response = self.read_full_response()?;
        let status = response[0];
        let data = &response[1..];

        if data.is_empty() {
            return Ok(None);
        }

        match status {
            STATUS_SUCCESS => self.parse_key_value_response(data, key_len).map(Some),
            STATUS_ERROR => Err(ClientError::OperationFailed(String::from_utf8_lossy(data).into_owned())),
            _ => Err(ClientError::InvalidResponse(format!("Unexpected status: {}", status)))
        }
    }

    pub fn next_dup_val(&mut self, table_code: u8, cursor_id: Vec<u8>) -> Result<Option<Vec<u8>>, ClientError> {
        let mut request = vec![OP_NEXT_DUP_VAL];
        request.extend_from_slice(&table_code.to_be_bytes());
        request.extend_from_slice(&cursor_id);
        
        println!("NEXT DUP VAL REQUEST: {:?}", request);
        self.stream.write_all(&request)?;
        self.stream.flush()?;

        let response = self.read_full_response()?;
        let status = response[0];
        let data = &response[1..];

        if data.is_empty() {
            return Ok(None);
        }

        match status {
            STATUS_SUCCESS => Ok(Some(data.to_vec())),
            STATUS_ERROR => Err(ClientError::OperationFailed(String::from_utf8_lossy(data).into_owned())),
            _ => Err(ClientError::InvalidResponse(format!("Unexpected status: {}", status)))
        }
    }

    pub fn seek_by_key_subkey(&mut self, table_code: u8, cursor_id: Vec<u8>, key: &[u8], subkey: &[u8]) -> Result<Option<Vec<u8>>, ClientError> {
        let mut request = vec![OP_SEEK_BY_KEY_SUBKEY];
        request.extend_from_slice(&table_code.to_be_bytes());
        request.extend_from_slice(&cursor_id);
        request.extend_from_slice(key);
        request.extend_from_slice(subkey);
        
        println!("SEEK BY KEY SUBKEY REQUEST: {:?}", request);
        self.stream.write_all(&request)?;
        self.stream.flush()?;

        let response = self.read_full_response()?;
        let status = response[0];
        let data = &response[1..];

        if data.is_empty() {
            return Ok(None);
        }

        match status {
            STATUS_SUCCESS => Ok(Some(data.to_vec())),
            STATUS_ERROR => Err(ClientError::OperationFailed(String::from_utf8_lossy(data).into_owned())),
            _ => Err(ClientError::InvalidResponse(format!("Unexpected status: {}", status)))
        }
    }

    pub fn delete_current_duplicates(&mut self, table_code: u8, cursor_id: Vec<u8>) -> Result<(), ClientError> {
        let mut request = vec![OP_DELETE_CURRENT_DUPLICATE];
        request.extend_from_slice(&table_code.to_be_bytes());
        request.extend_from_slice(&cursor_id);
        
        println!("DELETE CURRENT DUPLICATES REQUEST: {:?}", request);
        self.stream.write_all(&request)?;
        self.stream.flush()?;

        let response = self.read_full_response()?;
        let status = response[0];
        let data: &[u8] = &response[1..];

        match status {
            STATUS_SUCCESS => Ok(()),
            STATUS_ERROR => Err(ClientError::OperationFailed(String::from_utf8_lossy(data).into_owned())),
            _ => Err(ClientError::InvalidResponse(format!("Unexpected status: {}", status)))
        }
    }

    pub fn append_dup(&mut self, table_code: u8, cursor_id: Vec<u8>, key: &[u8], subkey: &[u8], value: &[u8]) -> Result<(), ClientError> {
        let mut request = vec![OP_APPEND_DUP, table_code];
        request.extend_from_slice(&cursor_id);
        
        request.extend_from_slice(key);
        request.extend_from_slice(subkey);
        request.extend_from_slice(value);
        
        println!("APPEND DUP REQUEST: {:?}", request);
        self.stream.write_all(&request)?;
        self.stream.flush()?;
    
        let response = self.read_full_response()?;
        println!("RESPONSE FOR APPEND DUP: {:?}", response);

        let status = response[0];
        let data = response[1..].to_vec();

        match status {
            STATUS_SUCCESS => Ok(()),
            STATUS_ERROR => Err(ClientError::OperationFailed(String::from_utf8_lossy(&data).into_owned())),
            _ => Err(ClientError::OperationFailed(format!("Error: {:?}", data)))
        }
    }

    pub fn check_additional_messages(&mut self) {
        println!("Checking for additional messages...");
        // Set socket to non-blocking mode for checking additional messages
        self.stream.set_nonblocking(true).unwrap_or_else(|e| println!("Failed to set non-blocking mode: {}", e));
        
        loop {
            let mut buffer = vec![0u8; 4096];
            match self.stream.read(&mut buffer) {
                Ok(n) if n > 0 => {
                    buffer.truncate(n);
                    println!("Additional message received: {:?}", buffer);
                }
                Ok(_) => {
                    println!("No more messages");
                    break;
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    println!("No more messages");
                    break;
                }
                Err(e) => {
                    println!("Error reading additional messages: {}", e);
                    break;
                }
            }
        }
        
        // Set socket back to blocking mode
        self.stream.set_nonblocking(false).unwrap_or_else(|e| println!("Failed to set blocking mode: {}", e));
    }

    fn parse_key_value_response(&self, data: &[u8], key_len_in_bytes: u32) -> Result<(Vec<u8>, Vec<u8>), ClientError> {
        if data.len() < key_len_in_bytes.try_into().unwrap() {
            return Err(ClientError::InvalidResponse("Response too short for key length".to_string()));
        }
        
        if data.len() < key_len_in_bytes.try_into().unwrap() {
            return Err(ClientError::InvalidResponse("Response too short for key".to_string()));
        }
        
        let key = data[0..key_len_in_bytes as usize].to_vec();
        let value = data[key_len_in_bytes as usize..].to_vec();
        Ok((key, value))
    }
}

pub fn generate_unique_bytes() -> [u8; 8] {
    // Get current timestamp with nanosecond precision
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64;
    
    // Generate random number for additional entropy
    let random = thread_rng().gen::<u32>() as u64;
    
    // Combine timestamp and random data
    let unique_num = (timestamp << 32) | random;
    
    // Convert to bytes
    unique_num.to_be_bytes()
}

impl std::fmt::Debug for ScalerizeClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScalerizeClient")
            .field("stream", &format!("UnixStream connected to {}", SOCKET_PATH))
            .finish()
    }
}