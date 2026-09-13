pub mod bitbucket_cloud;
pub mod bitbucket_data_center;
pub mod github;

use std::collections::BTreeMap;

use crate::application::ports::{ProviderCatalog, ProviderFailure};
use crate::domain::config::Config;
use crate::domain::repository::{RepositoryRef, RepositorySummary};
use bitbucket_cloud::BitbucketCloud;
use bitbucket_data_center::BitbucketDataCenter;
use github::Github;

#[derive(Debug)]
enum Adapter {
    Github(Github),
    BitbucketCloud(BitbucketCloud),
    BitbucketDataCenter(BitbucketDataCenter),
}

#[derive(Debug, Default)]
pub struct ConfiguredProviders {
    adapters: BTreeMap<String, Adapter>,
}

impl ConfiguredProviders {
    pub fn from_config(config: &Config) -> Result<Self, String> {
        let mut adapters = BTreeMap::new();
        for (host, provider) in &config.providers {
            let preset = provider.preset.as_str();
            let adapter = match preset {
                "github" => Adapter::Github(Github::new(host.clone(), provider.clone())),
                "bitbucket-cloud" => {
                    Adapter::BitbucketCloud(BitbucketCloud::new(host.clone(), provider.clone())?)
                }
                "bitbucket-data-center" => Adapter::BitbucketDataCenter(BitbucketDataCenter::new(
                    host.clone(),
                    provider.clone(),
                )?),
                other => return Err(format!("unknown provider preset `{other}` for `{host}`")),
            };
            adapters.insert(host.to_ascii_lowercase(), adapter);
        }
        Ok(Self { adapters })
    }

    fn adapter(&self, host: &str) -> Result<&Adapter, String> {
        self.adapters
            .get(&host.to_ascii_lowercase())
            .ok_or_else(|| format!("no configured provider for `{host}`"))
    }
}

impl ProviderCatalog for ConfiguredProviders {
    fn catalog(&self, include_archived: bool) -> Result<Vec<RepositorySummary>, String> {
        let (repositories, failures) = self.catalog_with_failures(include_archived);
        if failures.is_empty() {
            Ok(repositories)
        } else {
            Err(failures
                .into_iter()
                .map(|failure| format!("{}: {}", failure.provider, failure.error))
                .collect::<Vec<_>>()
                .join("; "))
        }
    }

    fn catalog_with_failures(
        &self,
        include_archived: bool,
    ) -> (Vec<RepositorySummary>, Vec<ProviderFailure>) {
        if self.adapters.is_empty() {
            return (
                Vec::new(),
                vec![ProviderFailure {
                    provider: "catalog".to_owned(),
                    error: "no configured providers".to_owned(),
                }],
            );
        }
        let mut repositories = Vec::new();
        let mut failures = Vec::new();
        for (host, adapter) in &self.adapters {
            match call_catalog(adapter, include_archived) {
                Ok(items) => {
                    repositories.extend(items.into_iter().map(|item| apply_prefix(item, adapter)))
                }
                Err(error) => failures.push(ProviderFailure {
                    provider: host.clone(),
                    error,
                }),
            }
        }
        (repositories, failures)
    }

    fn expand(
        &self,
        reference: &RepositoryRef,
        include_archived: bool,
    ) -> Result<Vec<RepositorySummary>, String> {
        let adapter = self.adapter(&reference.host)?;
        let items = match adapter {
            Adapter::Github(provider) => provider.expand(reference, include_archived),
            Adapter::BitbucketCloud(provider) => provider.expand(reference, include_archived),
            Adapter::BitbucketDataCenter(provider) => provider.expand(reference, include_archived),
        }?;
        Ok(items
            .into_iter()
            .map(|item| apply_prefix(item, adapter))
            .collect())
    }
}

fn apply_prefix(mut summary: RepositorySummary, adapter: &Adapter) -> RepositorySummary {
    let (prefix, data_center) = match adapter {
        Adapter::Github(provider) => (&provider.config.prefix, false),
        Adapter::BitbucketCloud(provider) => (&provider.config.prefix, false),
        Adapter::BitbucketDataCenter(provider) => (&provider.config.prefix, true),
    };
    let mut remote_path: Vec<String> = summary
        .reference
        .path
        .split('/')
        .map(ToOwned::to_owned)
        .collect();
    if data_center && let Some(project) = remote_path.first_mut() {
        *project = project.strip_prefix('~').unwrap_or(project).to_owned();
    }
    let mut destination: Vec<String> = prefix
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(ToOwned::to_owned)
        .collect();
    destination.extend(remote_path);
    summary.reference.destination_segments = destination;
    summary
}

fn call_catalog(
    adapter: &Adapter,
    include_archived: bool,
) -> Result<Vec<RepositorySummary>, String> {
    match adapter {
        Adapter::Github(provider) => provider.catalog(include_archived),
        Adapter::BitbucketCloud(provider) => provider.catalog(include_archived),
        Adapter::BitbucketDataCenter(provider) => provider.catalog(include_archived),
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::collections::HashMap;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::Duration;

    pub struct HttpFixture {
        pub base: String,
        requests: Arc<Mutex<Vec<(String, String)>>>,
    }

    impl HttpFixture {
        pub fn new(routes: Vec<(String, String)>) -> Self {
            let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind fixture");
            listener
                .set_nonblocking(true)
                .expect("configure fixture listener");
            let base = format!("http://{}", listener.local_addr().expect("fixture address"));
            let requests = Arc::new(Mutex::new(Vec::new()));
            let recorded = Arc::clone(&requests);
            let fixture_base = base.clone();
            thread::spawn(move || {
                let routes: HashMap<_, _> = routes
                    .into_iter()
                    .map(|(path, body)| (path, body.replace("{BASE}", &fixture_base)))
                    .collect();
                loop {
                    let (mut stream, _) = match listener.accept() {
                        Ok(connection) => connection,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(2));
                            continue;
                        }
                        Err(_) => break,
                    };
                    let _ = stream.set_nonblocking(false);
                    let mut bytes = Vec::new();
                    let mut buffer = [0; 1024];
                    loop {
                        let count = stream.read(&mut buffer).unwrap_or(0);
                        if count == 0 {
                            break;
                        }
                        bytes.extend_from_slice(&buffer[..count]);
                        if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
                            break;
                        }
                    }
                    let request = String::from_utf8_lossy(&bytes);
                    let mut lines = request.lines();
                    let path = lines
                        .next()
                        .and_then(|line| line.split_whitespace().nth(1))
                        .unwrap_or("/")
                        .to_owned();
                    let authorization = request
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("authorization")
                                .then(|| value.trim())
                        })
                        .unwrap_or("")
                        .to_owned();
                    recorded
                        .lock()
                        .expect("fixture lock")
                        .push((path.clone(), authorization));
                    let (status, body) = routes
                        .get(&path)
                        .map(|body| ("200 OK", body.as_str()))
                        .unwrap_or(("404 Not Found", "{}"));
                    let response = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes());
                }
            });
            Self { base, requests }
        }

        pub fn requests(&self) -> Vec<(String, String)> {
            self.requests.lock().expect("fixture lock").clone()
        }
    }
}
