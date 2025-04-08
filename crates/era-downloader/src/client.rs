use futures::StreamExt;
use reqwest::{Client, IntoUrl, Url};
use std::{path::Path, str::FromStr};
use tokio::{fs::File, sync::mpsc};

#[derive(Debug)]
pub struct EraClient {
    client: Client,
    max_files: usize,
    max_concurrent_downloads: usize,
    url: Url,
    folder: Box<Path>,
    processed_files: mpsc::Receiver<Box<Path>>,
    downloaded_files: mpsc::Sender<Box<Path>>,
    pending: usize,
}

impl EraClient {
    pub async fn download(&mut self) -> eyre::Result<()> {
        loop {
            let count = self.files_count().await.saturating_sub(self.max_files);

            for _ in 0..count {
                if let Some(url) = self.next_url().await {
                    self.pending += 1;
                    let path = self.folder.to_path_buf();

                    let url = url.into_url().unwrap();
                    let client = self.client.clone();
                    let file_name = url.path_segments().unwrap().last().unwrap();
                    let path = path.join(file_name);
                    let channel = self.downloaded_files.clone();

                    tokio::spawn(async move {
                        let response = client.get(url).send().await;

                        if let Ok(response) = response {
                            let mut stream = response.bytes_stream();
                            let mut file = File::create(path).await.unwrap();

                            while let Some(item) = stream.next().await {
                                tokio::io::copy(&mut item.unwrap().as_ref(), &mut file)
                                    .await
                                    .unwrap();
                            }

                            channel.send(path.into_boxed_path()).await.unwrap();
                        }
                    });
                }
            }

            if self.pending == 0 {
                break;
            }

            if let Some(file) = self.processed_files.recv().await {
                self.delete_file(&file).await?;
            }
        }

        Ok(())
    }

    async fn next_url(&self) -> Option<impl IntoUrl> {
        let mut max = None;

        for mut dir in tokio::fs::read_dir(&self.folder).await {
            for entry in dir.next_entry().await {
                if let Some(entry) = entry {
                    if let Some(name) = entry.file_name().to_str() {
                        if let Some(number) = self.file_name_to_number(name) {
                            if max.is_none() || number > max.unwrap() {
                                max.replace(number);
                            }
                        }
                    }
                }
            }
        }

        let number = max.unwrap_or(0);

        Some(self.number_to_file_name(number))
    }

    async fn files_count(&self) -> usize {
        tokio::fs::read_dir(&self.folder).await.iter().count()
    }

    async fn delete_file(&self, path: &Path) -> Result<(), std::io::Error> {
        tokio::fs::remove_file(path).await
    }

    fn number_to_file_name(&self, number: u64) -> String {
        String::new()
    }

    fn file_name_to_number(&self, file_name: &str) -> Option<u64> {
        file_name.split("-").skip(1).next().map(|v| u64::from_str(v).ok()).flatten()
    }
}
