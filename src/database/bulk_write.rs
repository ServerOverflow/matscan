//! An implementation of bulk writes for mongodb since the official driver
//! doesn't support it yet.
//!
//! Based on https://docs.rs/mongodm/latest/mongodm/trait.CollectionExt.html#tymethod.bulk_update

use std::borrow::Borrow;

use async_trait::async_trait;
use bson::{doc, oid::ObjectId, to_bson, Document};
use mongodb::options::UpdateOptions;
use serde::Deserialize;

/// Represents an individual update operation for the `bulk_update` function.
#[derive(Debug, Clone)]
pub struct BulkUpdate {
    pub query: Document,
    pub update: Document,
    pub options: Option<UpdateOptions>,
}

/// Result of a `bulk_update` operation.
#[derive(Debug, Deserialize)]
pub struct BulkUpdateResult {
    #[serde(rename = "n")]
    pub nb_affected: u64,
    #[serde(rename = "nModified")]
    pub nb_modified: u64,
    #[serde(default)]
    pub upserted: Vec<BulkUpdateUpsertResult>,
}

/// Individual update result of a `bulk_update` operation.
/// Contains the generated id in case of an upsert.
#[derive(Debug, Deserialize)]
pub struct BulkUpdateUpsertResult {
    pub index: u64,
    #[serde(alias = "_id")]
    pub id: ObjectId,
}

#[async_trait]
pub trait CollectionExt {
    async fn bulk_update<'async_trait, V, U>(
        &self,
        db: &mongodb::Database,
        updates: V,
    ) -> anyhow::Result<BulkUpdateResult>
    where
        V: 'async_trait + Send + Sync + Borrow<Vec<U>>,
        U: 'async_trait + Send + Sync + Borrow<BulkUpdate>;
}

#[async_trait]
impl<M: Send + Sync> CollectionExt for mongodb::Collection<M> {
    async fn bulk_update<'async_trait, V, U>(
        &self,
        db: &mongodb::Database,
        updates: V,
    ) -> anyhow::Result<BulkUpdateResult>
    where
        V: 'async_trait + Send + Sync + Borrow<Vec<U>>,
        U: 'async_trait + Send + Sync + Borrow<BulkUpdate>,
    {
        let updates = updates.borrow();
        let mut update_docs = Vec::with_capacity(updates.len());
        for u in updates {
            let u = u.borrow();
            let mut doc = doc! {
                "q": &u.query,
                "u": &u.update,
                "multi": false,
            };
            if let Some(options) = &u.options {
                if let Some(upsert) = &options.upsert {
                    doc.insert("upsert", upsert);
                }
                if let Some(collation) = &options.collation {
                    doc.insert("collation", to_bson(collation)?);
                }
                if let Some(array_filters) = &options.array_filters {
                    doc.insert("arrayFilters", array_filters);
                }
                if let Some(hint) = &options.hint {
                    doc.insert("hint", to_bson(hint)?);
                }
            }
            update_docs.push(doc);
        }
        let mut command = doc! {
            "update": self.name(),
            "updates": update_docs,
            "ordered": false,
        };
        if let Some(ref write_concern) = self.write_concern() {
            command.insert("writeConcern", to_bson(write_concern)?);
        }
        let res = db.run_command(command).await?;

        Ok(bson::from_document(res)?)
    }
}
