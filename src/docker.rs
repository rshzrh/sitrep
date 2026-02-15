use bollard::Docker;
use bollard::container::{
    ListContainersOptions, StatsOptions, LogsOptions, LogOutput, Stats,
    StopContainerOptions, RestartContainerOptions,
};
use bollard::models::ContainerSummary;
use futures_util::StreamExt;
use tokio::sync::mpsc;

use crate::model::DockerContainerInfo;

/// Wrapper around bollard's Docker client.
pub struct DockerClient {
    client: Docker,
}

impl DockerClient {
    /// Try to connect to the Docker daemon.
    /// Returns None if Docker is not available.
    pub fn try_new() -> Option<Self> {
        let client = Docker::connect_with_local_defaults().ok()?;
        Some(Self { client })
    }

    /// Ping the daemon to verify it is reachable.
    pub async fn is_available(&self) -> bool {
        self.client.ping().await.is_ok()
    }

    /// List running containers and map them to our model type.
    pub async fn list_containers(&self) -> Vec<DockerContainerInfo> {
        let options: ListContainersOptions<String> = ListContainersOptions {
            all: false, // only running
            ..Default::default()
        };

        let summaries = match self.client.list_containers(Some(options)).await {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        let mut containers = Vec::new();
        for s in summaries {
            let info = self.summary_to_info(&s).await;
            containers.push(info);
        }
        containers
    }

    /// Fetch a one-shot stats snapshot for a container. Returns cpu_percent.
    pub async fn get_cpu_percent(&self, container_id: &str) -> f64 {
        let options = StatsOptions {
            stream: false,
            one_shot: true,
        };

        let mut stream = self.client.stats(container_id, Some(options));
        if let Some(Ok(stats)) = stream.next().await {
            calculate_cpu_percent(&stats)
        } else {
            0.0
        }
    }

    /// Start streaming logs for a container. Returns a receiver channel.
    /// The sender task runs in the background on the provided tokio runtime handle.
    pub fn tail_logs(
        &self,
        container_id: &str,
        handle: &tokio::runtime::Handle,
    ) -> mpsc::Receiver<String> {
        let (tx, rx) = mpsc::channel::<String>(256);

        let options: LogsOptions<String> = LogsOptions {
            stdout: true,
            stderr: true,
            follow: true,
            tail: "200".to_string(),
            timestamps: true,
            ..Default::default()
        };

        let stream = self.client.logs(container_id, Some(options));

        handle.spawn(async move {
            let mut stream = Box::pin(stream);
            while let Some(result) = stream.next().await {
                let line = match result {
                    Ok(LogOutput::StdOut { message }) => {
                        String::from_utf8_lossy(&message).trim_end().to_string()
                    }
                    Ok(LogOutput::StdErr { message }) => {
                        String::from_utf8_lossy(&message).trim_end().to_string()
                    }
                    Ok(LogOutput::Console { message }) => {
                        String::from_utf8_lossy(&message).trim_end().to_string()
                    }
                    Ok(LogOutput::StdIn { message: _ }) => continue,
                    Err(e) => {
                        let _ = tx.send(format!("[error] {}", e)).await;
                        break;
                    }
                };
                if tx.send(line).await.is_err() {
                    break; // receiver dropped
                }
            }
        });

        rx
    }

    /// Start a stopped container.
    pub async fn start_container(&self, container_id: &str) -> Result<(), String> {
        self.client
            .start_container::<String>(container_id, None)
            .await
            .map_err(|e| e.to_string())
    }

    /// Stop a running container.
    pub async fn stop_container(&self, container_id: &str) -> Result<(), String> {
        let options = StopContainerOptions { t: 10 };
        self.client
            .stop_container(container_id, Some(options))
            .await
            .map_err(|e| e.to_string())
    }

    /// Restart a container.
    pub async fn restart_container(&self, container_id: &str) -> Result<(), String> {
        let options = RestartContainerOptions { t: 10 };
        self.client
            .restart_container(container_id, Some(options))
            .await
            .map_err(|e| e.to_string())
    }

    // --- Internal helpers ---

    async fn summary_to_info(&self, s: &ContainerSummary) -> DockerContainerInfo {
        let id_full = s.id.clone().unwrap_or_default();
        let id_short = id_full.chars().take(12).collect::<String>();

        let name = s.names.as_ref()
            .and_then(|n| n.first())
            .map(|n| n.trim_start_matches('/').to_string())
            .unwrap_or_else(|| id_short.clone());

        let image = s.image.clone().unwrap_or_default();

        let state = s.state.clone().unwrap_or_default();
        let status = s.status.clone().unwrap_or_default();

        let uptime = format_uptime(s.created.unwrap_or(0));

        let ports = format_ports(s);

        let ip_address = extract_ip(s);

        DockerContainerInfo {
            id: id_short,
            name,
            image,
            status,
            state,
            uptime,
            cpu_percent: 0.0, // filled separately via stats
            ports,
            ip_address,
        }
    }
}

// --- Free helper functions ---

fn calculate_cpu_percent(stats: &Stats) -> f64 {
    let cpu_stats = &stats.cpu_stats;
    let precpu_stats = &stats.precpu_stats;

    let cpu_delta = cpu_stats.cpu_usage.total_usage as f64
        - precpu_stats.cpu_usage.total_usage as f64;
    let system_delta = cpu_stats.system_cpu_usage.unwrap_or(0) as f64
        - precpu_stats.system_cpu_usage.unwrap_or(0) as f64;

    if system_delta > 0.0 && cpu_delta > 0.0 {
        let num_cpus = cpu_stats.online_cpus.unwrap_or(1) as f64;
        (cpu_delta / system_delta) * num_cpus * 100.0
    } else {
        0.0
    }
}

fn format_uptime(created_ts: i64) -> String {
    if created_ts == 0 {
        return "unknown".to_string();
    }
    let now = chrono::Utc::now().timestamp();
    let secs = (now - created_ts).max(0) as u64;

    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86400 {
        let h = secs / 3600;
        let m = (secs % 3600) / 60;
        format!("{}h {}m", h, m)
    } else {
        let d = secs / 86400;
        let h = (secs % 86400) / 3600;
        format!("{}d {}h", d, h)
    }
}

fn format_ports(s: &ContainerSummary) -> String {
    let Some(ports) = &s.ports else { return String::new() };
    let mut parts = Vec::new();
    for p in ports {
        let container_port = p.private_port;
        let proto = p.typ.as_ref()
            .map(|t| t.to_string())
            .unwrap_or_else(|| "tcp".to_string());
        if let (Some(ip), Some(pub_port)) = (&p.ip, p.public_port) {
            parts.push(format!("{}:{}->{}/{}", ip, pub_port, container_port, proto));
        } else {
            parts.push(format!("{}/{}", container_port, proto));
        }
    }
    parts.join(", ")
}

fn extract_ip(s: &ContainerSummary) -> String {
    if let Some(settings) = &s.network_settings {
        if let Some(networks) = &settings.networks {
            // Return the first network's IP
            for (_name, endpoint) in networks {
                if let Some(ip) = &endpoint.ip_address {
                    if !ip.is_empty() {
                        return ip.clone();
                    }
                }
            }
        }
    }
    String::new()
}
