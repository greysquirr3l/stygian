//! File-based template storage adapter

use crate::Result;
use crate::domain::ExtractionTemplate;
use crate::error::PluginError;
use crate::ports::PluginTemplateStore;
use async_trait::async_trait;
use std::path::PathBuf;
use tokio::fs;

/// File-based template store
///
/// Stores each template as a JSON file in a directory.
/// Template ID becomes the filename (UUID).
///
/// # Example
///
/// ```no_run
/// use stygian_plugin::storage::FileTemplateStore;
/// use std::path::PathBuf;
///
/// let store = FileTemplateStore::new(PathBuf::from("./templates"));
/// ```
pub struct FileTemplateStore {
    /// Directory to store template JSON files
    templates_dir: PathBuf,
}

impl FileTemplateStore {
    /// Create a new file-based template store
    pub const fn new(templates_dir: PathBuf) -> Self {
        Self { templates_dir }
    }

    /// Get the file path for a template ID
    fn template_path(&self, id: &uuid::Uuid) -> PathBuf {
        self.templates_dir.join(format!("{id}.json"))
    }

    /// Ensure the templates directory exists
    async fn ensure_dir(&self) -> Result<()> {
        fs::create_dir_all(&self.templates_dir).await.map_err(|e| {
            PluginError::StorageError(format!("Failed to create templates dir: {e}"))
        })?;
        Ok(())
    }
}

#[async_trait]
impl PluginTemplateStore for FileTemplateStore {
    async fn save(&self, template: &ExtractionTemplate) -> Result<()> {
        self.ensure_dir().await?;
        template.validate()?;

        let path = self.template_path(&template.id);
        let json =
            serde_json::to_string_pretty(template).map_err(PluginError::SerializationError)?;

        fs::write(&path, json)
            .await
            .map_err(|e| PluginError::StorageError(format!("Failed to write template: {e}")))?;

        Ok(())
    }

    async fn get(&self, id: &uuid::Uuid) -> Result<ExtractionTemplate> {
        let path = self.template_path(id);

        let content = fs::read_to_string(&path)
            .await
            .map_err(|_| PluginError::TemplateNotFound(id.to_string()))?;

        serde_json::from_str(&content).map_err(PluginError::SerializationError)
    }

    async fn list(&self) -> Result<Vec<ExtractionTemplate>> {
        self.ensure_dir().await?;

        let mut templates = Vec::new();
        let mut entries = fs::read_dir(&self.templates_dir)
            .await
            .map_err(|e| PluginError::StorageError(format!("Failed to read templates dir: {e}")))?;

        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| PluginError::StorageError(format!("Failed to read dir entry: {e}")))?
        {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "json") {
                match fs::read_to_string(&path).await {
                    Ok(content) => match serde_json::from_str::<ExtractionTemplate>(&content) {
                        Ok(template) => templates.push(template),
                        Err(e) => {
                            tracing::warn!("Failed to parse template {}: {}", path.display(), e);
                        }
                    },
                    Err(e) => {
                        tracing::warn!("Failed to read template {}: {}", path.display(), e);
                    }
                }
            }
        }

        Ok(templates)
    }

    async fn delete(&self, id: &uuid::Uuid) -> Result<()> {
        let path = self.template_path(id);

        fs::remove_file(&path)
            .await
            .map_err(|_| PluginError::TemplateNotFound(id.to_string()))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Region, Selector};
    use serde_json::json;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_save_and_get_template() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let tmp = TempDir::new()?;
        let store = FileTemplateStore::new(tmp.path().to_path_buf());

        let region = Region::new("test", Selector::css(".test"), json!({"type": "string"}));
        let template = ExtractionTemplate::new("Test Template").with_region(region);
        let id = template.id;

        store.save(&template).await?;
        let retrieved = store.get(&id).await?;

        assert_eq!(retrieved.id, id);
        assert_eq!(retrieved.name, "Test Template");
        Ok(())
    }

    #[tokio::test]
    async fn test_list_templates() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let tmp = TempDir::new()?;
        let store = FileTemplateStore::new(tmp.path().to_path_buf());

        let region = Region::new("test", Selector::css(".test"), json!({"type": "string"}));
        let t1 = ExtractionTemplate::new("Template 1").with_region(region.clone());
        let t2 = ExtractionTemplate::new("Template 2").with_region(region);

        store.save(&t1).await?;
        store.save(&t2).await?;

        let list = store.list().await?;
        assert_eq!(list.len(), 2);
        Ok(())
    }

    #[tokio::test]
    async fn test_delete_template() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let tmp = TempDir::new()?;
        let store = FileTemplateStore::new(tmp.path().to_path_buf());

        let region = Region::new("test", Selector::css(".test"), json!({"type": "string"}));
        let template = ExtractionTemplate::new("Test").with_region(region);
        let id = template.id;

        store.save(&template).await?;
        store.delete(&id).await?;

        let result = store.get(&id).await;
        assert!(result.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn test_search_templates() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let tmp = TempDir::new()?;
        let store = FileTemplateStore::new(tmp.path().to_path_buf());

        let region = Region::new("test", Selector::css(".test"), json!({"type": "string"}));
        let t1 = ExtractionTemplate::new("Product Scraper").with_region(region.clone());
        let t2 = ExtractionTemplate::new("Review Extractor").with_region(region);

        store.save(&t1).await?;
        store.save(&t2).await?;

        let results = store.search("product").await?;
        assert_eq!(results.len(), 1);
        let first = results.first().ok_or("expected at least one result")?;
        assert_eq!(first.name, "Product Scraper");
        Ok(())
    }
}
