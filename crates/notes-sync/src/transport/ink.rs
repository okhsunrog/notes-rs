use super::*;
use notes_core::ink::transfer::{self, Blobs};
use notes_protocol::ink::{self as wire, Hashes};
use std::collections::BTreeMap;

impl HttpTransport {
    pub async fn missing_ink_blobs(&self, hashes: Vec<BlobHash>) -> Result<Vec<BlobHash>> {
        let mut missing = vec![];
        for hashes in hashes.chunks(wire::MAX_HASHES) {
            let response = self
                .client
                .post(self.endpoint("v1/ink/missing")?)
                .bearer_auth(&self.token)
                .json(&Hashes {
                    hashes: hashes.to_vec(),
                })
                .send()
                .await?;
            let reply: Hashes = require_success(response).await?.json().await?;
            reply.validate()?;
            anyhow::ensure!(
                reply.hashes.iter().all(|h| hashes.contains(h)),
                "Server requested an unknown ink blob"
            );
            missing.extend(reply.hashes);
        }
        Ok(missing)
    }
    pub async fn upload_ink_blobs(&self, blobs: Blobs) -> Result<()> {
        for batch in wire::batches(blobs)? {
            let response = self
                .client
                .post(self.endpoint("v1/ink/upload")?)
                .bearer_auth(&self.token)
                .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
                .body(wire::encode(&batch)?)
                .send()
                .await?;
            require_success(response).await?;
        }
        Ok(())
    }
    /// Returns only requested verified blobs, with bounded response buffering.
    pub async fn download_ink_blobs(&self, hashes: Vec<BlobHash>) -> Result<Blobs> {
        Hashes {
            hashes: hashes.clone(),
        }
        .validate()?;
        let response = self
            .client
            .post(self.endpoint("v1/ink/download")?)
            .bearer_auth(&self.token)
            .json(&Hashes {
                hashes: hashes.clone(),
            })
            .send()
            .await?;
        let mut response = require_success(response).await?;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            anyhow::ensure!(
                bytes.len() + chunk.len() <= wire::MAX_BATCH_BYTES,
                "Oversized ink download"
            );
            bytes.extend(chunk);
        }
        let blobs = wire::decode(&bytes)?;
        anyhow::ensure!(
            blobs.len() == hashes.len() && hashes.iter().all(|h| blobs.contains_key(h)),
            "Incomplete ink download"
        );
        Ok(blobs)
    }
    pub async fn upload_ink_graph(
        &self,
        connection: &notes_core::Connection,
        root: BlobHash,
    ) -> Result<usize> {
        let mut blobs = transfer::export_graph(connection, root).await?;
        let missing = self
            .missing_ink_blobs(blobs.keys().copied().collect())
            .await?;
        let mut send = Blobs::new();
        for hash in missing {
            send.insert(hash, blobs.remove(&hash).context("Missing local ink blob")?);
        }
        let count = send.len();
        self.upload_ink_blobs(send).await?;
        Ok(count)
    }
    pub async fn download_ink_graph(
        &self,
        connection: &notes_core::Connection,
        root_hash: BlobHash,
    ) -> Result<usize> {
        let mut blobs = transfer::cached_blobs(connection, vec![root_hash]).await?;
        let mut downloaded = 0;
        if !blobs.contains_key(&root_hash) {
            blobs.extend(self.download_ink_blobs(vec![root_hash]).await?);
            downloaded += 1;
        }
        let required = transfer::required_blobs(root_hash, &blobs[&root_hash])?;
        blobs.extend(transfer::cached_blobs(connection, required.keys().copied().collect()).await?);
        let missing = required
            .into_iter()
            .filter(|(hash, _)| !blobs.contains_key(hash))
            .collect::<BTreeMap<_, _>>();
        let mut batch = Vec::new();
        let mut size = 12;
        for (hash, len) in missing {
            let added = 36 + len as usize;
            if batch.len() == wire::MAX_HASHES || size + added > wire::MAX_BATCH_BYTES {
                downloaded += batch.len();
                blobs.extend(self.download_ink_blobs(std::mem::take(&mut batch)).await?);
                size = 12;
            }
            size += added;
            batch.push(hash);
        }
        if !batch.is_empty() {
            downloaded += batch.len();
            blobs.extend(self.download_ink_blobs(batch).await?);
        }
        transfer::stage_graph(connection, root_hash, blobs).await?;
        Ok(downloaded)
    }
}
