use moq_net::announce::Kind;
use std::collections::HashMap;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Button {
    Up,
    Down,
    Left,
    Right,
}

impl Button {
    const ALL: [Button; 4] = [Button::Up, Button::Down, Button::Left, Button::Right];
}

#[derive(serde::Deserialize, Debug)]
struct RawTimestamp {
    label: String,
    /// Media timestamp in milliseconds.
    ts: f64,
}

/// A parsed timestamp entry with Duration.
#[derive(Debug)]
pub struct TimestampEntry {
    pub label: String,
    pub ts: Duration,
}

#[derive(serde::Deserialize, Debug)]
#[serde(tag = "type")]
enum RawCommand {
    #[serde(rename = "buttons")]
    Buttons {
        #[serde(default)]
        buttons: Vec<Button>,
        /// Ordered media timestamps at each pipeline stage.
        #[serde(default)]
        timestamps: Vec<RawTimestamp>,
    },
    #[serde(rename = "reset")]
    Reset {},
}

#[derive(Debug)]
pub enum Command {
    /// Full button state for a viewer.
    Buttons {
        buttons: Vec<Button>,
        viewer_id: String,
        /// Ordered media timestamps at each pipeline stage.
        timestamps: Vec<TimestampEntry>,
    },
    Reset,
    /// A viewer disconnected or went offline.
    ViewerLeft {
        viewer_id: String,
    },
}

pub async fn handle_viewers(
    viewer_origin: &moq_net::origin::Consumer,
    scenes: &HashMap<String, tokio::sync::mpsc::Sender<Command>>,
) -> anyhow::Result<()> {
    let mut announced = viewer_origin.announced();
    // The command reader per viewer, so a restarted viewer replaces the old one's.
    let mut readers: HashMap<String, tokio::task::AbortHandle> = Default::default();
    while let Some(update) = announced.next().await {
        let path = update.prefix.to_string();
        let Some((scene, viewer_id)) = path.split_once("/") else {
            continue;
        };
        if viewer_id.is_empty() || viewer_id.contains("/") {
            continue;
        }
        let Some(cmd_tx) = scenes.get(scene) else {
            tracing::debug!(%scene, %viewer_id, "viewer for unknown scene");
            continue;
        };
        match update.kind {
            Kind::Announced | Kind::Updated => {
                if readers.get(&path).is_some_and(|r| !r.is_finished()) {
                    continue;
                }
                let broadcast = match viewer_origin.request_broadcast(&update.prefix).await {
                    Ok(b) => b,
                    Err(e) => {
                        tracing::warn!(%viewer_id, %scene, "failed to resolve scene");
                        continue;
                    }
                };
                tracing::info!(%scene, %viewer_id, "viewer connected");
                if let Some(old) = readers.remove(&path) {
                    old.abort();
                }
                let vid = viewer_id.to_string();
                let cmd_tx = cmd_tx.clone();
                let reader = tokio::spawn(async move {
                    if let Err(e) = handle_viewer_commands(&vid, broadcast, &cmd_tx).await {
                        tracing::warn!(viewer_id = %vid,error = %e, "viewer command error");
                    }
                    let _ = cmd_tx
                        .send(Command::ViewerLeft {
                            viewer_id: vid.to_string(),
                        })
                        .await;
                });
                readers.insert(path.clone(), reader.abort_handle());
            }
            Kind::Retracted => {
                if let Some(old) = readers.remove(&path) {
                    old.abort();
                }
                tracing::info!(%viewer_id , %scene,  "viewer is offline");
                let _ = cmd_tx
                    .send(Command::ViewerLeft {
                        viewer_id: viewer_id.to_string(),
                    })
                    .await;
            }
        }
    }
    Ok(())
}

async fn handle_viewer_commands(
    viewer_id: &str,
    broadcast: moq_net::broadcast::Consumer,
    cmd_tx: &tokio::sync::mpsc::Sender<Command>,
) -> anyhow::Result<()> {
    let track = broadcast.track("command")?.subscribe(None).await?;
    let mut commands = moq_json::snapshot::Consumer::<RawCommand>::new(
        track,
        moq_json::snapshot::consumer::Config::default(),
    );

    loop {
        let command = match commands.next().await {
            Ok(Some(command)) => command,
            Ok(None) => break,
            // A malformed command is skipped; a track error ends the stream.
            Err(moq_json::Error::Json(err)) => {
                tracing::warn!(%viewer_id, %err, "invalid command");
                continue;
            }
            Err(err) => return Err(err.into()),
        };

        match command {
            RawCommand::Buttons {
                buttons,
                timestamps,
            } => {
                let timestamps: Vec<_> = timestamps
                    .into_iter()
                    .filter_map(|t| {
                        let ts = Duration::try_from_secs_f64(t.ts / 1000.0).ok()?;
                        Some(TimestampEntry { label: t.label, ts })
                    })
                    .collect();
                let _ = cmd_tx
                    .send(Command::Buttons {
                        buttons,
                        viewer_id: viewer_id.to_string(),
                        timestamps,
                    })
                    .await;
            }
            RawCommand::Reset {} => {
                tracing::info!(%viewer_id, "reset");
                let _ = cmd_tx.send(Command::Reset).await;
            }
        }
    }

    Ok(())
}
