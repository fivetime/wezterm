use clap::Parser;
use mux::pane::PaneId;
use wezterm_client::client::Client;

#[derive(Debug, Parser, Clone)]
pub struct ActivatePane {
    /// Specify the target pane.
    /// The default is to use the current pane based on the
    /// environment variable WEZTERM_PANE.
    #[arg(long)]
    pane_id: Option<PaneId>,
}

impl ActivatePane {
    pub async fn run(&self, client: Client) -> anyhow::Result<()> {
        let pane_id = client.resolve_pane_id(self.pane_id).await?;
        hand_on_activation_token(&client).await;
        client
            .set_focused_pane_id(codec::SetFocusedPane { pane_id })
            .await?;
        Ok(())
    }
}

/// The activation token the program that runs this was given for the
/// window to come forward with (Wayland), handed on to the GUI first.
pub async fn hand_on_activation_token(client: &Client) {
    let Ok(token) = std::env::var("XDG_ACTIVATION_TOKEN") else {
        return;
    };
    if token.is_empty() {
        return;
    }
    if let Err(e) = client
        .set_activation_token(codec::SetActivationToken { token })
        .await
    {
        log::debug!("the activation token was not handed on: {e:#}");
    }
}
