use std::cell::RefCell;
use std::fs;
use std::rc::Rc;
use std::time::Instant;

use picobot::cli::format_permissions;
use picobot::cli::tui::{ModelChoice, PermissionChoice, Tui, TuiEvent};
use picobot::config::Config;
use picobot::kernel::agent::Kernel;
use picobot::kernel::agent_loop::{
    ConversationState, PermissionDecision, run_agent_loop_streamed_with_permissions_limit,
};
use picobot::kernel::permissions::CapabilitySet;
use picobot::models::router::ModelRegistry;
use picobot::tools::builtin::register_builtin_tools;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = load_config().unwrap_or_default();

    let registry = match ModelRegistry::from_config(&config) {
        Ok(registry) => registry,
        Err(err) => {
            eprintln!("Model registry error: {err}");
            return Ok(());
        }
    };

    let tool_registry = register_builtin_tools(config.permissions.as_ref())?;
    let working_dir = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let capabilities = config
        .permissions
        .as_ref()
        .map(CapabilitySet::from_config)
        .unwrap_or_else(CapabilitySet::empty);
    let kernel = Kernel::new(tool_registry, working_dir).with_capabilities(capabilities);

    let mut state = ConversationState::new();
    if let Some(agent) = &config.agent
        && let Some(system_prompt) = &agent.system_prompt
    {
        state.push(picobot::models::types::Message::system(
            system_prompt.clone(),
        ));
    }

    run_tui(&kernel, &registry, &mut state, config.permissions.as_ref()).await
}

async fn run_tui(
    kernel: &Kernel,
    registry: &ModelRegistry,
    state: &mut ConversationState,
    permissions: Option<&picobot::config::PermissionsConfig>,
) -> anyhow::Result<()> {
    let tui = Rc::new(RefCell::new(
        Tui::new().map_err(|err| anyhow::anyhow!(err))?,
    ));
    let mut current_model_id = registry.default_id().to_string();
    {
        if let Ok(mut ui) = tui.try_borrow_mut() {
            if let Some(model) = registry.get(&current_model_id) {
                let info = model.info();
                ui.set_current_model(format!("{} ({})", info.provider, info.model));
            }
            ui.refresh().ok();
        }
    }
    loop {
        let event = {
            let mut ui = tui.borrow_mut();
            ui.next_event().map_err(|err| anyhow::anyhow!(err))?
        };
        match event {
            TuiEvent::Quit => break,
            TuiEvent::Input(line) => {
                if line == "/clear" {
                    let mut ui = tui.borrow_mut();
                    ui.clear_output();
                    ui.refresh().ok();
                    continue;
                }
                if line == "/permissions" {
                    let mut ui = tui.borrow_mut();
                    ui.push_output(format_permissions(permissions));
                    ui.refresh().ok();
                    continue;
                }
                if line == "/models" {
                    let models = registry
                        .model_infos()
                        .into_iter()
                        .map(|info| ModelChoice {
                            id: info.id.clone(),
                            label: format!("{}: {} ({})", info.id, info.provider, info.model),
                        })
                        .collect();
                    let mut ui = tui.borrow_mut();
                    ui.set_pending_model_picker(models);
                    ui.refresh().ok();
                    continue;
                }
                if line.trim().is_empty() {
                    continue;
                }

                {
                    let mut ui = tui.borrow_mut();
                    ui.push_user(format!("> {line}"));
                    ui.start_assistant_message();
                    ui.set_busy(true);
                    ui.refresh().ok();
                }

                let mut buffer = String::new();
                let tui_token = Rc::clone(&tui);
                let tui_permission = Rc::clone(&tui);
                let tui_debug = Rc::clone(&tui);
                let last_flush = Rc::new(RefCell::new(Instant::now()));
                let last_flush_token = Rc::clone(&last_flush);
                let last_flush_debug = Rc::clone(&last_flush);
                let model = registry
                    .get(&current_model_id)
                    .unwrap_or_else(|| registry.default_model());
                let result = run_agent_loop_streamed_with_permissions_limit(
                    kernel,
                    model,
                    state,
                    line,
                    &mut |token| {
                        buffer.push_str(token);
                        if let Ok(mut ui) = tui_token.try_borrow_mut() {
                            ui.append_output(token);
                            if last_flush_token.borrow().elapsed().as_millis() > 30 {
                                ui.refresh().ok();
                                *last_flush_token.borrow_mut() = Instant::now();
                            }
                        }
                    },
                    &mut |tool, required| {
                        let permissions = required.iter().map(|perm| format!("{perm:?}")).collect();
                        if let Ok(mut ui) = tui_permission.try_borrow_mut() {
                            ui.set_busy(false);
                            ui.set_pending_permission(tool.to_string(), permissions);
                            ui.refresh().ok();
                        }
                        loop {
                            let event = if let Ok(mut ui) = tui_permission.try_borrow_mut() {
                                ui.next_event().ok()
                            } else {
                                None
                            };
                            match event {
                                Some(TuiEvent::Permission(choice)) => {
                                    if let Ok(mut ui) = tui_permission.try_borrow_mut() {
                                        ui.clear_pending_permission();
                                        ui.set_busy(true);
                                        ui.refresh().ok();
                                    }
                                    return match choice {
                                        PermissionChoice::Once => PermissionDecision::Once,
                                        PermissionChoice::Session => PermissionDecision::Session,
                                        PermissionChoice::Deny => PermissionDecision::Deny,
                                    };
                                }
                                Some(TuiEvent::Quit) => {
                                    if let Ok(mut ui) = tui_permission.try_borrow_mut() {
                                        ui.clear_pending_permission();
                                        ui.set_busy(false);
                                        ui.refresh().ok();
                                    }
                                    return PermissionDecision::Deny;
                                }
                                Some(_) => {}
                                None => {}
                            }
                        }
                    },
                    &mut |line| {
                        if let Ok(mut ui) = tui_debug.try_borrow_mut() {
                            ui.push_debug(line.to_string());
                            if last_flush_debug.borrow().elapsed().as_millis() > 30 {
                                ui.refresh().ok();
                                *last_flush_debug.borrow_mut() = Instant::now();
                            }
                        }
                    },
                    8,
                )
                .await;
                if let Ok(mut ui) = tui.try_borrow_mut() {
                    ui.set_busy(false);
                    if let Err(err) = result {
                        ui.push_output(format!("Error: {err}"));
                    } else if !buffer.ends_with('\n') {
                        ui.push_assistant("".to_string());
                    }
                    ui.refresh().ok();
                }
            }
            TuiEvent::ModelPick(model_id) => {
                current_model_id = model_id.clone();
                if let Ok(mut ui) = tui.try_borrow_mut() {
                    if let Some(model) = registry.get(&current_model_id) {
                        let info = model.info();
                        ui.set_current_model(format!("{} ({})", info.provider, info.model));
                        ui.push_output(format!("Switched to model '{}'", info.id));
                    } else {
                        ui.push_output("Unknown model id".to_string());
                    }
                    ui.clear_pending_model_picker();
                    ui.refresh().ok();
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn load_config() -> Option<Config> {
    let raw = fs::read_to_string("config.toml").ok()?;
    toml::from_str(&raw).ok()
}
