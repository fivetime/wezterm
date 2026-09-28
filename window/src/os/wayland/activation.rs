//! A window brought to the front on Wayland: only with an activation
//! token (xdg-activation-v1), which the program that has the person's
//! input asks the compositor for and hands on. As Chromium does
//! (`ui/ozone/platform/wayland/host/xdg_activation.cc`,
//! `base/nix/xdg_util.cc`, `wayland_toplevel_window.cc`): one token kept
//! for the process, from `XDG_ACTIVATION_TOKEN` at start (taken out of
//! the environment then, so that no program started from here is given
//! it) or handed on from another process (`wezterm cli`); a window asked
//! to come forward takes it, and one not configured yet takes it when it
//! is.

use smithay_client_toolkit::activation::{ActivationHandler, RequestData};
use std::sync::Mutex;

use super::state::WaylandState;

const ENV: &str = "XDG_ACTIVATION_TOKEN";

static TOKEN: Mutex<Option<String>> = Mutex::new(None);

fn token() -> std::sync::MutexGuard<'static, Option<String>> {
    TOKEN.lock().unwrap_or_else(|e| e.into_inner())
}

/// The token the next window asked to come forward is activated with.
pub fn set_activation_token(token_: String) {
    if !token_.is_empty() {
        *token() = Some(token_);
    }
}

pub(super) fn take_activation_token() -> Option<String> {
    token().take()
}

/// The token this process was started with, if any.
pub(super) fn token_from_env() {
    if let Ok(given) = std::env::var(ENV) {
        std::env::remove_var(ENV);
        set_activation_token(given);
    }
}

impl ActivationHandler for WaylandState {
    type RequestData = RequestData;

    // (tokens asked for here: none are, the asking program's are used)
    fn new_token(&mut self, token: String, data: &Self::RequestData) {
        if let (Some(activation), Some(surface)) = (&self.activation, data.surface.as_ref()) {
            activation.activate::<WaylandState>(surface, token);
        }
    }
}

smithay_client_toolkit::delegate_activation!(WaylandState);
