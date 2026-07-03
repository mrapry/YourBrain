//! Vector index abstraction + a dependency-free flat (brute-force) backend.
//!
//! The engine talks only to the [`VectorIndex`] trait. The default [`FlatIndex`]
//! keeps all vectors in memory and scores by cosine similarity — exact results,
//! zero native dependencies, fine for the tens-of-thousands scale a personal /
//! small-team brain reaches. A HNSW backend (usearch) can be swapped in behind
//! this trait later (see ADR-1/ADR-18) without touching callers.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::Path;

use crate::embed::cosine;
use crate::error::{Result, YbError};

/// A nearest-neighbour index over memory embeddings, keyed by memory id (ULID).
pub trait VectorIndex: Send {
    /// Insert or replace the vector for `id`.
    fn upsert(&mut self, id: &str, vector: Vec<f32>);
    /// Remove the vector for `id` (no-op if absent).
    fn remove(&mut self, id: &str);
    /// Return the top-`k` ids by cosine similarity, descending.
    fn search(&self, query: &[f32], top_k: usize) -> Vec<(String, f32)>;
    /// Number of vectors currently indexed.
    fn len(&self) -> usize;
    /// Whether the index is empty.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Exact brute-force cosine index with a simple binary on-disk format.
#[derive(Debug, Default, Clone)]
pub struct FlatIndex {
    dim: usize,
    vectors: HashMap<String, Vec<f32>>,
}

const MAGIC: &[u8; 4] = b"YBV1";

impl FlatIndex {
    pub fn new(dim: usize) -> Self {
        Self {
            dim,
            vectors: HashMap::new(),
        }
    }

    pub fn dimension(&self) -> usize {
        self.dim
    }

    /// Load an index from disk, or return an empty index of `dim` if missing.
    pub fn open_or_create(path: &Path, dim: usize) -> Result<Self> {
        if path.exists() {
            Self::load(path)
        } else {
            Ok(Self::new(dim))
        }
    }

    /// Persist the index to `path` using a compact binary layout:
    /// `MAGIC | dim:u32 | count:u64 | [ id_len:u16 | id | dim*f32(le) ]*`.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut w = BufWriter::new(File::create(path)?);
        w.write_all(MAGIC)?;
        w.write_all(&(self.dim as u32).to_le_bytes())?;
        w.write_all(&(self.vectors.len() as u64).to_le_bytes())?;
        for (id, vec) in &self.vectors {
            let id_bytes = id.as_bytes();
            w.write_all(&(id_bytes.len() as u16).to_le_bytes())?;
            w.write_all(id_bytes)?;
            for &f in vec {
                w.write_all(&f.to_le_bytes())?;
            }
        }
        w.flush()?;
        Ok(())
    }

    /// Load an index previously written by [`FlatIndex::save`].
    pub fn load(path: &Path) -> Result<Self> {
        let mut r = BufReader::new(File::open(path)?);
        let mut magic = [0u8; 4];
        r.read_exact(&mut magic)?;
        if &magic != MAGIC {
            return Err(YbError::other("invalid vector index file (bad magic)"));
        }
        let dim = read_u32(&mut r)? as usize;
        let count = read_u64(&mut r)? as usize;
        let mut vectors = HashMap::with_capacity(count);
        for _ in 0..count {
            let id_len = read_u16(&mut r)? as usize;
            let mut id_buf = vec![0u8; id_len];
            r.read_exact(&mut id_buf)?;
            let id = String::from_utf8(id_buf)
                .map_err(|_| YbError::other("invalid utf8 id in vector index"))?;
            let mut vec = vec![0f32; dim];
            let mut fbuf = [0u8; 4];
            for slot in vec.iter_mut() {
                r.read_exact(&mut fbuf)?;
                *slot = f32::from_le_bytes(fbuf);
            }
            vectors.insert(id, vec);
        }
        Ok(Self { dim, vectors })
    }
}

fn read_u16<R: Read>(r: &mut R) -> Result<u16> {
    let mut b = [0u8; 2];
    r.read_exact(&mut b)?;
    Ok(u16::from_le_bytes(b))
}
fn read_u32<R: Read>(r: &mut R) -> Result<u32> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b)?;
    Ok(u32::from_le_bytes(b))
}
fn read_u64<R: Read>(r: &mut R) -> Result<u64> {
    let mut b = [0u8; 8];
    r.read_exact(&mut b)?;
    Ok(u64::from_le_bytes(b))
}

impl VectorIndex for FlatIndex {
    fn upsert(&mut self, id: &str, vector: Vec<f32>) {
        self.vectors.insert(id.to_string(), vector);
    }

    fn remove(&mut self, id: &str) {
        self.vectors.remove(id);
    }

    fn search(&self, query: &[f32], top_k: usize) -> Vec<(String, f32)> {
        let mut scored: Vec<(String, f32)> = self
            .vectors
            .iter()
            .map(|(id, v)| (id.clone(), cosine(query, v)))
            .collect();
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(top_k);
        scored
    }

    fn len(&self) -> usize {
        self.vectors.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embed::{Embedder, HashEmbedder};

    #[test]
    fn upsert_search_remove() {
        let e = HashEmbedder::default();
        let mut idx = FlatIndex::new(e.dimension());
        idx.upsert("a", e.embed("authentication with JWT and Redis"));
        idx.upsert("b", e.embed("kubernetes deployment on GCP"));
        idx.upsert("c", e.embed("JWT auth tokens stored in Redis cache"));

        let q = e.embed("how does JWT authentication work with Redis");
        let res = idx.search(&q, 2);
        assert_eq!(res.len(), 2);
        // The two auth-related memories should rank above the k8s one.
        let ids: Vec<&str> = res.iter().map(|(id, _)| id.as_str()).collect();
        assert!(ids.contains(&"a") || ids.contains(&"c"));
        assert!(!ids.contains(&"b"));

        idx.remove("a");
        assert_eq!(idx.len(), 2);
    }

    #[test]
    fn persistence_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("brain.ybv");
        let e = HashEmbedder::default();
        let mut idx = FlatIndex::new(e.dimension());
        idx.upsert("01ABC", e.embed("hello world"));
        idx.upsert("01DEF", e.embed("goodbye world"));
        idx.save(&path).unwrap();

        let loaded = FlatIndex::load(&path).unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded.dimension(), e.dimension());
        let q = e.embed("hello");
        assert!(!loaded.search(&q, 1).is_empty());
    }
}
