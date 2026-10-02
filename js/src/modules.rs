//! ES module loading for page realms: HTTP(S)/virtual-scheme resolution and
//! synchronous fetch through the engine network stack (privacy policy,
//! cache and cookies all apply).
//!
//! Supported specifiers: absolute URLs and relative/absolute paths resolved
//! against the importing module's URL. Bare specifiers (node_modules-style)
//! are intentionally unsupported — browsers don't support them either.

use rquickjs::loader::{ImportAttributes, Loader, Resolver};
use rquickjs::module::Declared;
use rquickjs::{Ctx, Error, Module, Result};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct HttpModuleResolver {
    pub base_url: String,
}

impl Resolver for HttpModuleResolver {
    fn resolve<'js>(
        &mut self,
        _ctx: &Ctx<'js>,
        base: &str,
        name: &str,
        _attributes: Option<ImportAttributes<'js>>,
    ) -> Result<String> {
        // Absolute specifier (has a scheme): use as-is.
        if url::Url::parse(name).is_ok() {
            return Ok(name.to_string());
        }
        // Relative to the importing module (or the document base for the
        // entry module, where `base` is empty).
        let effective_base = if base.is_empty() { self.base_url.as_str() } else { base };
        let base_url = url::Url::parse(effective_base)
            .map_err(|e| Error::new_resolving_message(effective_base, name, e.to_string()))?;
        let resolved = base_url
            .join(name)
            .map_err(|e| Error::new_resolving_message(effective_base, name, e.to_string()))?;
        Ok(resolved.to_string())
    }
}

pub struct HttpModuleLoader {
    net: Arc<brows12_net::HttpClient>,
    tokio: Arc<tokio::runtime::Runtime>,
    cache: Mutex<HashMap<String, String>>,
}

impl HttpModuleLoader {
    pub fn new(
        net: Arc<brows12_net::HttpClient>,
        tokio: Arc<tokio::runtime::Runtime>,
        _base_url: String,
    ) -> Self {
        HttpModuleLoader { net, tokio, cache: Mutex::new(HashMap::new()) }
    }

    fn fetch_source(&self, url: &str) -> Result<String> {
        if let Some(cached) = self.cache.lock().unwrap().get(url) {
            return Ok(cached.clone());
        }
        let parsed = url::Url::parse(url).map_err(|e| Error::new_resolving_message(url, "", e.to_string()))?;
        let top_site = brows12_storage::registrable_domain(parsed.host_str().unwrap_or(""));
        let request = brows12_net::NetRequest::get(url.to_string(), top_site);
        let response = self
            .tokio
            .block_on(self.net.send(request))
            .map_err(|e| Error::new_resolving_message(url, "", e.to_string()))?;
        if !response.is_success() {
            return Err(Error::new_resolving_message(url, "", format!("status {}", response.status)));
        }
        let body = String::from_utf8_lossy(&response.body).to_string();
        self.cache.lock().unwrap().insert(url.to_string(), body.clone());
        Ok(body)
    }
}

impl Loader for HttpModuleLoader {
    fn load<'js>(
        &mut self,
        ctx: &Ctx<'js>,
        name: &str,
        _attributes: Option<ImportAttributes<'js>>,
    ) -> Result<Module<'js, Declared>> {
        let source = self.fetch_source(name)?;
        Module::declare(ctx.clone(), name.to_string(), source)
    }
}
