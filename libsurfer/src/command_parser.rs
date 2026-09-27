//! Command prompt handling.
use regex::Regex;
use std::sync::LazyLock;
use std::{fs, str::FromStr};

use camino::Utf8PathBuf;

use crate::config::ArrowKeyBindings;
use crate::displayed_item_tree::{Node, VisibleItemIndex};
use crate::frame_buffer::FrameBufferColorMode;
use crate::fzcmd::{Command, ParamGreed};
use crate::hierarchy::HierarchyStyle;
use crate::message::MessageTarget;
use crate::transaction_container::StreamScopeRef;
use crate::user_signals::{UserSignals, WaveEditSnap, parse_user_value};
use crate::wave_container::{ScopeRef, ScopeRefExt, VariableRef, VariableRefExt};
use crate::wave_data::ScopeType;
use crate::wave_source::LoadOptions;
use crate::{
    SystemState,
    clock_highlighting::ClockHighlightType,
    displayed_item::{AnalogRenderStyle, AnalogSettings, DisplayedItem},
    message::Message,
    toolbar::toolbar_group_specs,
    util::{alpha_idx_to_uint_idx, uint_idx_to_alpha_idx},
    variable_name_type::VariableNameType,
};
use itertools::Itertools;
use tracing::warn;

type RestCommand = Box<dyn Fn(&str) -> Option<Command<Message>>>;

/// Match str with wave file extensions, currently: vcd, fst, ghw
fn is_wave_file_extension(ext: &str) -> bool {
    matches!(ext, "vcd" | "fst" | "ghw")
}

/// Match str with command file extensions, currently: sucl
fn is_command_file_extension(ext: &str) -> bool {
    matches!(ext, "sucl")
}

/// Split part of a query at whitespace
///
/// fzcmd splits at regex "words" which does not include special characters
/// like '#'. This function can be used instead via `ParamGreed::Custom(&separate_at_space)`
fn separate_at_space(query: &str) -> (String, String, String, String) {
    static RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(\s*)(\S*)(\s?)(.*)").unwrap());

    let captures = RE.captures_iter(query).next().unwrap();

    (
        captures[1].into(),
        captures[2].into(),
        captures[3].into(),
        captures[4].into(),
    )
}

pub(crate) fn get_parser(state: &SystemState) -> Command<Message> {
    fn single_word(
        suggestions: Vec<String>,
        rest_command: RestCommand,
    ) -> Option<Command<Message>> {
        Some(Command::NonTerminal(
            ParamGreed::Rest,
            suggestions,
            Box::new(move |query, _| rest_command(query)),
        ))
    }

    fn optional_single_word(
        suggestions: Vec<String>,
        rest_command: RestCommand,
    ) -> Option<Command<Message>> {
        Some(Command::NonTerminal(
            ParamGreed::OptionalWord,
            suggestions,
            Box::new(move |query, _| rest_command(query)),
        ))
    }

    fn single_word_delayed_suggestions(
        suggestions: Box<dyn Fn() -> Vec<String>>,
        rest_command: RestCommand,
    ) -> Option<Command<Message>> {
        Some(Command::NonTerminal(
            ParamGreed::Rest,
            suggestions(),
            Box::new(move |query, _| rest_command(query)),
        ))
    }

    let scopes = match &state.user.waves {
        Some(v) => v.inner.scope_names(),
        None => vec![],
    };
    let variables = match &state.user.waves {
        Some(v) => v.inner.variable_names(),
        None => vec![],
    };
    let arrays = match &state.user.waves {
        Some(v) => v.inner.array_names(),
        None => vec![],
    };
    let surver_file_names = state
        .user
        .surver_file_infos
        .as_ref()
        .map_or(vec![], |file_infos| {
            file_infos
                .iter()
                .map(|info| info.filename.clone())
                .collect()
        });
    let displayed_items = match &state.user.waves {
        Some(v) => v
            .items_tree
            .iter_visible()
            .enumerate()
            .map(
                |(
                    vidx,
                    Node {
                        item_ref: item_id, ..
                    },
                )| {
                    let idx = VisibleItemIndex(vidx);
                    let item = &v.displayed_items[item_id];
                    match item {
                        DisplayedItem::Variable(var) => format!(
                            "{}_{}",
                            uint_idx_to_alpha_idx(idx, v.displayed_items.len()),
                            var.variable_ref.full_path_string()
                        ),
                        _ => format!(
                            "{}_{}",
                            uint_idx_to_alpha_idx(idx, v.displayed_items.len()),
                            item.name()
                        ),
                    }
                },
            )
            .collect_vec(),
        None => vec![],
    };
    let variables_in_active_scope = state
        .user
        .waves
        .as_ref()
        .and_then(|waves| {
            waves
                .active_scope
                .as_ref()
                .map(|scope| waves.inner.variables_in_scope(scope))
        })
        .unwrap_or_default();

    let color_names = state.user.config.theme.colors.keys().cloned().collect_vec();
    let format_names: Vec<String> = state
        .translators
        .all_translator_names()
        .into_iter()
        .map(&str::to_owned)
        .collect();
    let height_suggestions = state
        .user
        .config
        .layout
        .waveforms_line_height_multiples
        .iter()
        .map(ToString::to_string)
        .collect_vec();
    let active_scope = state
        .user
        .waves
        .as_ref()
        .and_then(|w| w.active_scope.clone());

    let is_transaction_container = state
        .user
        .waves
        .as_ref()
        .is_some_and(|w| w.inner.is_transactions());

    fn files_with_ext(matches: fn(&str) -> bool) -> Vec<String> {
        if let Ok(res) = fs::read_dir(".") {
            res.map(|res| res.map(|e| e.path()).unwrap_or_default())
                .filter(|file| {
                    file.extension()
                        .is_some_and(|extension| (matches)(extension.to_str().unwrap_or("")))
                })
                .map(|file| file.into_os_string().into_string().unwrap())
                .collect::<Vec<String>>()
        } else {
            vec![]
        }
    }

    fn all_wave_files() -> Vec<String> {
        files_with_ext(is_wave_file_extension)
    }

    fn all_command_files() -> Vec<String> {
        files_with_ext(is_command_file_extension)
    }

    let timescale = state
        .user
        .waves
        .as_ref()
        .map(|w| w.inner.metadata().timescale.clone());

    let viewport_idx = state
        .user
        .waves
        .as_ref()
        .map_or(0, |waves| waves.last_active_viewport_idx);
    let viewport_indices = state.user.waves.as_ref().map_or(vec![], |waves| {
        (0..waves.viewports.len())
            .map(|idx| idx.to_string())
            .collect()
    });

    let markers = if let Some(waves) = &state.user.waves {
        waves
            .items_tree
            .iter()
            .map(|Node { item_ref, .. }| waves.displayed_items.get(item_ref))
            .filter_map(|item| match item {
                Some(DisplayedItem::Marker(marker)) => Some((marker.name.clone(), marker.idx)),
                _ => None,
            })
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };

    fn parse_marker(query: &str, markers: &[(Option<String>, u8)]) -> Option<u8> {
        if let Some(id_str) = query.strip_prefix("#") {
            let id = id_str.parse::<u8>().ok()?;
            Some(id)
        } else {
            markers
                .iter()
                .find_map(|(name, idx)| name.as_ref().and_then(|n| (n == query).then_some(*idx)))
        }
    }

    fn marker_suggestions(markers: &[(Option<String>, u8)]) -> Vec<String> {
        markers
            .iter()
            .flat_map(|(name, idx)| {
                [name.clone(), Some(format!("#{idx}"))]
                    .into_iter()
                    .flatten()
            })
            .collect()
    }

    let wcp_start_or_stop = if state
        .wcp_running_signal
        .load(std::sync::atomic::Ordering::Relaxed)
    {
        "wcp_server_stop"
    } else {
        "wcp_server_start"
    };
    #[cfg(target_arch = "wasm32")]
    let _ = wcp_start_or_stop;

    let keep_during_reload = state.user.config.behavior.keep_during_reload;
    let toolbar_group_ids = toolbar_group_specs()
        .iter()
        .map(|spec| spec.id.to_string())
        .collect_vec();
    let mut commands = if state.user.waves.is_some() {
        vec![
            "load_file",
            "load_url",
            #[cfg(not(target_arch = "wasm32"))]
            "load_state",
            "run_command_file",
            "run_command_file_from_url",
            "switch_file",
            "variable_add",
            "generator_add",
            "item_focus",
            "item_set_color",
            "item_set_background_color",
            "item_set_format",
            "item_set_height",
            "item_set_analog",
            "item_unset_color",
            "item_unset_background_color",
            "item_unfocus",
            "item_rename",
            "zoom_fit",
            "scope_add",
            #[cfg(not(target_arch = "wasm32"))]
            "create_default_config",
            "scope_add_recursive",
            "scope_add_as_group",
            "scope_add_as_group_recursive",
            "scope_select",
            "scope_select_root",
            "stream_add",
            "stream_select",
            "stream_select_root",
            "divider_add",
            "config_reload",
            "theme_select",
            "reload",
            "remove_unavailable",
            "show_controls",
            "show_mouse_gestures",
            "show_quick_start",
            "show_logs",
            #[cfg(feature = "performance_plot")]
            "show_performance",
            "scroll_to_start",
            "scroll_to_end",
            "goto_start",
            "goto_end",
            "zoom_in",
            "zoom_out",
            "zoom_to",
            "toggle_menu",
            "toggle_side_panel",
            "toggle_fullscreen",
            "toggle_tick_lines",
            "toolbar_set_visible",
            "toolbar_set_row",
            "signal_create",
            "signal_set",
            "edit_mode_toggle",
            "edit_snap_to",
            "signal_revert",
            "signal_revert_all",
            "variable_add_from_scope",
            "generator_add_from_stream",
            "variable_set_name_type",
            "variable_force_name_type",
            "preference_set_clock_highlight",
            "preference_set_hierarchy_style",
            "preference_set_arrow_key_bindings",
            "goto_cursor",
            "goto_marker",
            "dump_tree",
            "group_marked",
            "group_dissolve",
            "group_fold_recursive",
            "group_unfold_recursive",
            "group_fold_all",
            "group_unfold_all",
            "save_state",
            "save_state_as",
            "timeline_add",
            "cursor_set",
            "goto_time",
            "marker_set",
            "marker_set_at",
            "marker_remove",
            "show_marker_window",
            "viewport_add",
            "viewport_remove",
            "viewport_set_active",
            "transition_next",
            "transition_previous",
            "transaction_next",
            "transaction_prev",
            "copy_value",
            "frame_buffer_set_array",
            "frame_buffer_set_variable",
            "frame_buffer_set_mode",
            "frame_buffer_set_width",
            "frame_buffer_set_range",
            "memory_viewer_open",
            "pause_simulation",
            "unpause_simulation",
            "undo",
            "redo",
            #[cfg(not(target_arch = "wasm32"))]
            wcp_start_or_stop,
            #[cfg(not(target_arch = "wasm32"))]
            "exit",
        ]
    } else {
        vec![
            "load_file",
            "load_url",
            #[cfg(not(target_arch = "wasm32"))]
            "load_state",
            "run_command_file",
            "run_command_file_from_url",
            "config_reload",
            "theme_select",
            "toggle_menu",
            "toggle_side_panel",
            "toggle_fullscreen",
            "toolbar_set_visible",
            "toolbar_set_row",
            "preference_set_clock_highlight",
            "preference_set_hierarchy_style",
            "preference_set_arrow_key_bindings",
            "show_controls",
            "show_mouse_gestures",
            "show_quick_start",
            "show_logs",
            #[cfg(not(target_arch = "wasm32"))]
            "create_default_config",
            #[cfg(feature = "performance_plot")]
            "show_performance",
            #[cfg(not(target_arch = "wasm32"))]
            wcp_start_or_stop,
            #[cfg(not(target_arch = "wasm32"))]
            "exit",
        ]
    };
    if !surver_file_names.is_empty() {
        commands.push("surver_select_file");
        commands.push("surver_switch_file");
    }

    let mut theme_names = state.user.config.theme.theme_names.clone();
    let state_file = state.user.state_file.clone();
    let show_hierarchy = state.show_hierarchy();
    let show_menu = state.show_menu();
    let wave_edit_mode = state.wave_edit_mode;
    let user_signal_names = state
        .user
        .waves
        .as_ref()
        .and_then(|w| w.user_signals())
        .map(|s| {
            s.variables()
                .into_iter()
                .map(|v| v.name)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let show_tick_lines = state.show_ticks();
    theme_names.insert(0, "default".to_string());
    Command::NonTerminal(
        ParamGreed::Word,
        commands.into_iter().map(std::convert::Into::into).collect(),
        Box::new(move |query, _| {
            let variables_in_active_scope = variables_in_active_scope.clone();
            let markers = markers.clone();
            let scopes = scopes.clone();
            let active_scope = active_scope.clone();
            let is_transaction_container = is_transaction_container;
            match query {
                "load_file" => single_word_delayed_suggestions(
                    Box::new(all_wave_files),
                    Box::new(|word| {
                        Some(Command::Terminal(Message::LoadFile(
                            word.into(),
                            LoadOptions::Clear,
                        )))
                    }),
                ),

                "create_default_config" => Some(Command::Terminal(Message::DownloadDefaultConfig)),
                "switch_file" => single_word_delayed_suggestions(
                    Box::new(all_wave_files),
                    Box::new(|word| {
                        Some(Command::Terminal(Message::LoadFile(
                            word.into(),
                            LoadOptions::KeepAll,
                        )))
                    }),
                ),
                "load_url" => Some(Command::NonTerminal(
                    ParamGreed::Rest,
                    vec![],
                    Box::new(|query, _| {
                        Some(Command::Terminal(Message::LoadWaveformFileFromUrl(
                            query.to_string(),
                            LoadOptions::Clear, // load_url does not indicate any format restrictions
                        )))
                    }),
                )),
                "run_command_file" => single_word_delayed_suggestions(
                    Box::new(all_command_files),
                    Box::new(|word| Some(Command::Terminal(Message::LoadCommandFile(word.into())))),
                ),
                "run_command_file_from_url" => Some(Command::NonTerminal(
                    ParamGreed::Rest,
                    vec![],
                    Box::new(|query, _| {
                        Some(Command::Terminal(Message::LoadCommandFileFromUrl(
                            query.to_string(),
                        )))
                    }),
                )),
                "config_reload" => Some(Command::Terminal(Message::ReloadConfig)),
                "theme_select" => single_word(
                    theme_names.clone(),
                    Box::new(|word| {
                        Some(Command::Terminal(Message::SelectTheme(Some(
                            word.to_owned(),
                        ))))
                    }),
                ),
                "scroll_to_start" | "goto_start" => {
                    Some(Command::Terminal(Message::GoToStart { viewport_idx }))
                }
                "scroll_to_end" | "goto_end" => {
                    Some(Command::Terminal(Message::GoToEnd { viewport_idx }))
                }
                "zoom_in" => Some(Command::Terminal(Message::CanvasZoom {
                    mouse_ptr: None,
                    delta: 0.5,
                    viewport_idx,
                })),
                "zoom_out" => Some(Command::Terminal(Message::CanvasZoom {
                    mouse_ptr: None,
                    delta: 2.0,
                    viewport_idx,
                })),
                "zoom_fit" => Some(Command::Terminal(Message::ZoomToFit { viewport_idx })),
                "zoom_to" => {
                    let timescale_for_zoom = timescale.clone();
                    Some(Command::NonTerminal(
                        ParamGreed::Rest,
                        vec![],
                        Box::new(move |params, _| {
                            let parts: Vec<&str> = params.split_whitespace().collect();
                            if parts.len() < 2 {
                                return None;
                            }

                            // Reconstruct time strings, handling optional spaces between number and unit
                            let mut time_strings = Vec::new();
                            let mut i = 0;
                            while i < parts.len() && time_strings.len() < 2 {
                                let part = parts[i];
                                // Check if this part is numeric (potentially followed by a unit in the next part)
                                if part
                                    .chars()
                                    .next()
                                    .is_some_and(|c| c.is_numeric() || c == '-')
                                {
                                    let time_str = if i + 1 < parts.len()
                                        && !parts[i + 1].chars().next().unwrap_or('0').is_numeric()
                                    {
                                        // Next part looks like a unit, combine them
                                        let combined = format!("{}{}", part, parts[i + 1]);
                                        i += 2;
                                        combined
                                    } else {
                                        // No unit following, use as-is
                                        i += 1;
                                        part.to_string()
                                    };
                                    time_strings.push(time_str);
                                } else {
                                    i += 1;
                                }
                            }

                            if time_strings.len() < 2 {
                                return None;
                            }

                            let start_time = if let Some(ts) = &timescale_for_zoom {
                                crate::time::parse_time_string_to_ticks(&time_strings[0], ts)?
                            } else {
                                time_strings[0].parse().ok()?
                            };

                            let end_time = if let Some(ts) = &timescale_for_zoom {
                                crate::time::parse_time_string_to_ticks(&time_strings[1], ts)?
                            } else {
                                time_strings[1].parse().ok()?
                            };

                            Some(Command::Terminal(Message::ZoomToRange {
                                start: start_time,
                                end: end_time,
                                viewport_idx,
                            }))
                        }),
                    ))
                }
                "toggle_menu" => Some(Command::Terminal(Message::SetMenuVisible(!show_menu))),
                // signal_revert <VARIABLE>
                "signal_revert" => single_word(
                    variables.clone(),
                    Box::new(|word| {
                        Some(Command::Terminal(Message::RevertSignalEdits(Some(
                            VariableRef::from_hierarchy_string(word),
                        ))))
                    }),
                ),
                "signal_revert_all" => Some(Command::Terminal(Message::RevertSignalEdits(None))),
                // edit_snap_to <ticks | VARIABLE>
                "edit_snap_to" => single_word(
                    std::iter::once("ticks".to_string())
                        .chain(variables.iter().cloned())
                        .collect(),
                    Box::new(|word| {
                        let snap = if word == "ticks" {
                            WaveEditSnap::Ticks
                        } else {
                            WaveEditSnap::Signal(VariableRef::from_hierarchy_string(word))
                        };
                        Some(Command::Terminal(Message::SetWaveEditSnap(snap)))
                    }),
                ),
                "edit_mode_toggle" => {
                    Some(Command::Terminal(Message::SetWaveEditMode(!wave_edit_mode)))
                }
                // signal_create <name> <width>
                "signal_create" => single_word(
                    vec![],
                    Box::new(|args| {
                        let [name, width] = args.split_whitespace().collect::<Vec<_>>()[..] else {
                            return None;
                        };
                        Some(Command::Terminal(Message::CreateUserSignal {
                            name: name.to_string(),
                            width: width.parse().ok()?,
                        }))
                    }),
                ),
                // signal_set <name> <start> <end or -> <value>, where <name> is a created
                // signal or the full path of a signal from the file, e.g. tb.dut.count
                "signal_set" => {
                    let timescale_for_signal_set = timescale.clone();
                    let created_names = user_signal_names.clone();
                    Some(Command::NonTerminal(
                        // Up to the next space, so a path like tb.count stays one parameter.
                        ParamGreed::Custom(&separate_at_space),
                        user_signal_names.clone(),
                        Box::new(move |name, _| {
                            let name = name.to_string();
                            let is_created = created_names.contains(&name);
                            let timescale = timescale_for_signal_set.clone();
                            single_word(
                                vec![],
                                Box::new(move |args| {
                                    let [start, end, value] =
                                        args.split_whitespace().collect::<Vec<_>>()[..]
                                    else {
                                        return None;
                                    };
                                    let parse_time = |t: &str| {
                                        if let Some(ts) = &timescale {
                                            crate::time::parse_time_string_to_ticks(t, ts)
                                        } else {
                                            t.parse().ok()
                                        }
                                    };
                                    let end = if end == "-" {
                                        None
                                    } else {
                                        Some(parse_time(end)?)
                                    };
                                    let variable = if is_created {
                                        VariableRef::new(UserSignals::scope(), name.clone())
                                    } else {
                                        VariableRef::from_hierarchy_string(&name)
                                    };
                                    Some(Command::Terminal(Message::SetSignalValue {
                                        variable,
                                        start: parse_time(start)?,
                                        end,
                                        value: parse_user_value(value, u32::MAX, 10).ok()?,
                                        continue_stroke: false,
                                    }))
                                }),
                            )
                        }),
                    ))
                }
                "toggle_side_panel" => Some(Command::Terminal(Message::SetSidePanelVisible(
                    !show_hierarchy,
                ))),
                "toggle_fullscreen" => Some(Command::Terminal(Message::ToggleFullscreen)),
                "toggle_tick_lines" => {
                    Some(Command::Terminal(Message::SetTickLines(!show_tick_lines)))
                }
                "toolbar_set_visible" => Some(Command::NonTerminal(
                    ParamGreed::Word,
                    toolbar_group_ids.clone(),
                    Box::new({
                        let toolbar_group_ids = toolbar_group_ids.clone();
                        move |word, _| {
                            if !toolbar_group_ids.iter().any(|id| id == word) {
                                return None;
                            }
                            let group_id = word.to_string();
                            Some(Command::NonTerminal(
                                ParamGreed::Word,
                                vec!["true".to_string(), "false".to_string()],
                                Box::new(move |value, _| {
                                    let enabled = match value {
                                        "true" => true,
                                        "false" => false,
                                        _ => return None,
                                    };
                                    Some(Command::Terminal(Message::SetToolbarGroupEnabled(
                                        group_id.clone(),
                                        enabled,
                                    )))
                                }),
                            ))
                        }
                    }),
                )),
                "toolbar_set_row" => Some(Command::NonTerminal(
                    ParamGreed::Word,
                    toolbar_group_ids.clone(),
                    Box::new({
                        let toolbar_group_ids = toolbar_group_ids.clone();
                        move |word, _| {
                            if !toolbar_group_ids.iter().any(|id| id == word) {
                                return None;
                            }
                            let group_id = word.to_string();
                            Some(Command::NonTerminal(
                                ParamGreed::Word,
                                vec![],
                                Box::new(move |value, _| {
                                    let row = value.parse::<u8>().ok()?;
                                    Some(Command::Terminal(Message::SetToolbarGroupRow(
                                        group_id.clone(),
                                        row,
                                    )))
                                }),
                            ))
                        }
                    }),
                )),
                // scope commands
                "scope_add" | "module_add" | "stream_add" | "scope_add_recursive" => {
                    let recursive = query == "scope_add_recursive";
                    if is_transaction_container {
                        if recursive {
                            warn!("Cannot recursively add transaction containers");
                        }
                        single_word(
                            scopes,
                            Box::new(|word| {
                                Some(Command::Terminal(Message::AddAllFromStreamScope(
                                    word.to_string(),
                                )))
                            }),
                        )
                    } else {
                        single_word(
                            scopes,
                            Box::new(move |word| {
                                Some(Command::Terminal(Message::AddScope(
                                    ScopeRef::from_hierarchy_string(word),
                                    recursive,
                                )))
                            }),
                        )
                    }
                }
                "scope_add_as_group" | "scope_add_as_group_recursive" => {
                    let recursive = query == "scope_add_as_group_recursive";
                    if is_transaction_container {
                        warn!("Cannot add transaction containers as group");
                        None
                    } else {
                        single_word(
                            scopes,
                            Box::new(move |word| {
                                Some(Command::Terminal(Message::AddScopeAsGroup(
                                    ScopeRef::from_hierarchy_string(word),
                                    recursive,
                                )))
                            }),
                        )
                    }
                }
                "scope_select" | "stream_select" => {
                    if is_transaction_container {
                        single_word(
                            scopes.clone(),
                            Box::new(|word| {
                                let scope = if word == "tr" {
                                    ScopeType::StreamScope(StreamScopeRef::Root)
                                } else {
                                    ScopeType::StreamScope(StreamScopeRef::Empty(word.to_string()))
                                };
                                Some(Command::Terminal(Message::SetActiveScope(Some(scope))))
                            }),
                        )
                    } else {
                        single_word(
                            scopes.clone(),
                            Box::new(|word| {
                                Some(Command::Terminal(Message::SetActiveScope(Some(
                                    ScopeType::WaveScope(ScopeRef::from_hierarchy_string(word)),
                                ))))
                            }),
                        )
                    }
                }
                "scope_select_root" | "stream_select_root" => {
                    Some(Command::Terminal(Message::SetActiveScope(None)))
                }
                "reload" => Some(Command::Terminal(Message::ReloadWaveform(
                    keep_during_reload,
                ))),
                "remove_unavailable" => Some(Command::Terminal(Message::RemovePlaceholders)),
                "surver_select_file" => single_word(
                    surver_file_names.clone(),
                    Box::new(|word| {
                        Some(Command::Terminal(Message::LoadSurverFileByName(
                            word.to_string(),
                            LoadOptions::Clear,
                        )))
                    }),
                ),
                "surver_switch_file" => single_word(
                    surver_file_names.clone(),
                    Box::new(|word| {
                        Some(Command::Terminal(Message::LoadSurverFileByName(
                            word.to_string(),
                            LoadOptions::KeepAll,
                        )))
                    }),
                ),
                // Variable commands
                "variable_add" | "generator_add" => {
                    if is_transaction_container {
                        single_word(
                            variables.clone(),
                            Box::new(|word| {
                                Some(Command::Terminal(Message::AddStreamOrGeneratorFromName(
                                    None,
                                    word.to_string(),
                                )))
                            }),
                        )
                    } else {
                        single_word(
                            variables.clone(),
                            Box::new(|word| {
                                Some(Command::Terminal(Message::AddVariables(vec![
                                    VariableRef::from_hierarchy_string(word),
                                ])))
                            }),
                        )
                    }
                }
                "variable_add_from_scope" | "generator_add_from_stream" => single_word(
                    variables_in_active_scope
                        .into_iter()
                        .map(|s| s.name_with_index())
                        .collect(),
                    Box::new(move |name| {
                        active_scope.as_ref().map(|scope| match scope {
                            ScopeType::WaveScope(w) => Command::Terminal(Message::AddVariables(
                                vec![VariableRef::new(w.clone(), name.to_string())],
                            )),
                            ScopeType::StreamScope(stream_scope) => {
                                Command::Terminal(Message::AddStreamOrGeneratorFromName(
                                    Some(stream_scope.clone()),
                                    name.to_string(),
                                ))
                            }
                        })
                    }),
                ),
                "item_set_color" => single_word(
                    color_names.clone(),
                    Box::new(|word| {
                        Some(Command::Terminal(Message::ItemColorChange(
                            MessageTarget::CurrentSelection,
                            Some(word.to_string()),
                        )))
                    }),
                ),
                "item_set_background_color" => single_word(
                    color_names.clone(),
                    Box::new(|word| {
                        Some(Command::Terminal(Message::ItemBackgroundColorChange(
                            MessageTarget::CurrentSelection,
                            Some(word.to_string()),
                        )))
                    }),
                ),
                "item_set_format" => single_word(
                    format_names.clone(),
                    Box::new(|word| {
                        Some(Command::Terminal(Message::VariableFormatChange(
                            MessageTarget::CurrentSelection,
                            word.to_string(),
                        )))
                    }),
                ),
                "item_set_height" => single_word(
                    height_suggestions.clone(),
                    Box::new(|word| {
                        let height = word.parse::<f32>().ok()?;
                        Some(Command::Terminal(Message::ItemHeightScalingFactorChange(
                            MessageTarget::CurrentSelection,
                            height,
                        )))
                    }),
                ),
                "item_set_analog" => single_word(
                    vec![
                        "off".to_string(),
                        "step".to_string(),
                        "interpolated".to_string(),
                    ],
                    Box::new(|word| {
                        let settings = match word {
                            "off" => None,
                            "step" => Some(AnalogSettings {
                                render_style: AnalogRenderStyle::Step,
                                ..Default::default()
                            }),
                            "interpolated" => Some(AnalogSettings {
                                render_style: AnalogRenderStyle::Interpolated,
                                ..Default::default()
                            }),
                            _ => return None,
                        };

                        Some(Command::Terminal(Message::SetAnalogSettings(
                            MessageTarget::CurrentSelection,
                            settings,
                        )))
                    }),
                ),
                "item_unset_color" => Some(Command::Terminal(Message::ItemColorChange(
                    MessageTarget::CurrentSelection,
                    None,
                ))),
                "item_unset_background_color" => Some(Command::Terminal(
                    Message::ItemBackgroundColorChange(MessageTarget::CurrentSelection, None),
                )),
                "item_rename" => Some(Command::NonTerminal(
                    ParamGreed::Rest,
                    vec![],
                    Box::new(|query, _| {
                        Some(Command::Terminal(Message::ItemNameChange(
                            None,
                            Some(query.to_owned()),
                        )))
                    }),
                )),
                "variable_set_name_type" => single_word(
                    vec![
                        "Local".to_string(),
                        "Unique".to_string(),
                        "Global".to_string(),
                    ],
                    Box::new(|word| {
                        Some(Command::Terminal(Message::ChangeVariableNameType(
                            MessageTarget::CurrentSelection,
                            VariableNameType::from_str(word).unwrap_or(VariableNameType::Local),
                        )))
                    }),
                ),
                "variable_force_name_type" => single_word(
                    vec![
                        "Local".to_string(),
                        "Unique".to_string(),
                        "Global".to_string(),
                    ],
                    Box::new(|word| {
                        Some(Command::Terminal(Message::ForceVariableNameTypes(
                            VariableNameType::from_str(word).unwrap_or(VariableNameType::Local),
                        )))
                    }),
                ),
                "item_focus" => single_word(
                    displayed_items.clone(),
                    Box::new(|word| {
                        // split off the idx which is always followed by an underscore
                        let alpha_idx: String = word.chars().take_while(|c| *c != '_').collect();
                        alpha_idx_to_uint_idx(&alpha_idx)
                            .map(|idx| Command::Terminal(Message::FocusItem(idx)))
                    }),
                ),
                "transition_next" => single_word(
                    displayed_items.clone(),
                    Box::new(|word| {
                        // split off the idx which is always followed by an underscore
                        let alpha_idx: String = word.chars().take_while(|c| *c != '_').collect();
                        alpha_idx_to_uint_idx(&alpha_idx).map(|idx| {
                            Command::Terminal(Message::MoveCursorToTransition {
                                next: true,
                                variable: Some(idx),
                                skip_zero: false,
                            })
                        })
                    }),
                ),
                "transition_previous" => single_word(
                    displayed_items.clone(),
                    Box::new(|word| {
                        // split off the idx which is always followed by an underscore
                        let alpha_idx: String = word.chars().take_while(|c| *c != '_').collect();
                        alpha_idx_to_uint_idx(&alpha_idx).map(|idx| {
                            Command::Terminal(Message::MoveCursorToTransition {
                                next: false,
                                variable: Some(idx),
                                skip_zero: false,
                            })
                        })
                    }),
                ),
                "transaction_next" => {
                    Some(Command::Terminal(Message::MoveTransaction { next: true }))
                }
                "transaction_prev" => {
                    Some(Command::Terminal(Message::MoveTransaction { next: false }))
                }
                "copy_value" => single_word(
                    displayed_items.clone(),
                    Box::new(|word| {
                        // split off the idx which is always followed by an underscore
                        let alpha_idx: String = word.chars().take_while(|c| *c != '_').collect();
                        alpha_idx_to_uint_idx(&alpha_idx).map(|idx| {
                            Command::Terminal(Message::VariableValueToClipbord(
                                MessageTarget::Explicit(idx),
                            ))
                        })
                    }),
                ),
                "preference_set_clock_highlight" => single_word(
                    ["Line", "Cycle", "None"]
                        .iter()
                        .map(ToString::to_string)
                        .collect_vec(),
                    Box::new(|word| {
                        Some(Command::Terminal(Message::SetClockHighlightType(
                            ClockHighlightType::from_str(word).unwrap_or(ClockHighlightType::Line),
                        )))
                    }),
                ),
                "preference_set_hierarchy_style" => single_word(
                    enum_iterator::all::<HierarchyStyle>()
                        .map(|o| o.to_string())
                        .collect_vec(),
                    Box::new(|word| {
                        Some(Command::Terminal(Message::SetHierarchyStyle(
                            HierarchyStyle::from_str(word).unwrap_or(HierarchyStyle::Separate),
                        )))
                    }),
                ),
                "preference_set_arrow_key_bindings" => single_word(
                    enum_iterator::all::<ArrowKeyBindings>()
                        .map(|o| o.to_string())
                        .collect_vec(),
                    Box::new(|word| {
                        Some(Command::Terminal(Message::SetArrowKeyBindings(
                            ArrowKeyBindings::from_str(word).unwrap_or(ArrowKeyBindings::Edge),
                        )))
                    }),
                ),
                "item_unfocus" => Some(Command::Terminal(Message::UnfocusItem)),
                "divider_add" => optional_single_word(
                    vec![],
                    Box::new(|word| {
                        Some(Command::Terminal(Message::AddDivider(
                            Some(word.into()),
                            None,
                        )))
                    }),
                ),
                "timeline_add" => Some(Command::Terminal(Message::AddTimeLine(None))),
                "goto_cursor" => Some(Command::Terminal(Message::GoToCursorIfNotInView)),
                "goto_marker" => single_word(
                    marker_suggestions(&markers),
                    Box::new(move |name| {
                        parse_marker(name, &markers)
                            .map(|idx| Command::Terminal(Message::GoToMarkerPosition(idx, 0)))
                    }),
                ),
                "frame_buffer_set_array" => single_word(
                    arrays.clone(),
                    Box::new(|word| {
                        Some(Command::Terminal(Message::SetFrameBufferArray(
                            ScopeRef::from_hierarchy_string(word),
                        )))
                    }),
                ),
                "frame_buffer_set_variable" => single_word(
                    variables.clone(),
                    Box::new(|word| {
                        Some(Command::Terminal(Message::SetFrameBufferVariable(
                            VariableRef::from_hierarchy_string(word),
                        )))
                    }),
                ),
                "frame_buffer_set_mode" => Some(Command::NonTerminal(
                    ParamGreed::Word,
                    vec![
                        "grayscale".to_string(),
                        "rgb".to_string(),
                        "ycbcr".to_string(),
                    ],
                    Box::new(|word, _| {
                        let mode = match word {
                            "grayscale" => FrameBufferColorMode::Grayscale,
                            "rgb" => FrameBufferColorMode::Rgb,
                            "ycbcr" => FrameBufferColorMode::YCbCr,
                            _ => return None,
                        };
                        Some(Command::NonTerminal(
                            ParamGreed::Rest,
                            vec![],
                            Box::new(move |rest, _| {
                                let args: Vec<&str> = rest.split_whitespace().collect();
                                match mode {
                                    FrameBufferColorMode::Grayscale => {
                                        if args.len() != 1 {
                                            return None;
                                        }
                                        let bits = args[0].parse::<u8>().ok()?;
                                        if !(1..=8).contains(&bits) {
                                            return None;
                                        }
                                        Some(Command::Terminal(Message::SetFrameBufferMode(
                                            mode, bits, 0, 0,
                                        )))
                                    }
                                    FrameBufferColorMode::Rgb | FrameBufferColorMode::YCbCr => {
                                        if args.len() != 3 {
                                            return None;
                                        }
                                        let bits1 = args[0].parse::<u8>().ok()?;
                                        let bits2 = args[1].parse::<u8>().ok()?;
                                        let bits3 = args[2].parse::<u8>().ok()?;
                                        if bits1 > 8 || bits2 > 8 || bits3 > 8 {
                                            return None;
                                        }
                                        Some(Command::Terminal(Message::SetFrameBufferMode(
                                            mode, bits1, bits2, bits3,
                                        )))
                                    }
                                }
                            }),
                        ))
                    }),
                )),
                "frame_buffer_set_width" => single_word(
                    vec![],
                    Box::new(|word| {
                        let width = word.parse::<usize>().ok()?.max(1);
                        Some(Command::Terminal(Message::SetFrameBufferWidth(width)))
                    }),
                ),
                "frame_buffer_set_range" => single_word(
                    vec![],
                    Box::new(|rest| {
                        let values: Vec<i64> = rest
                            .split_whitespace()
                            .map(str::parse::<i64>)
                            .collect::<Result<_, _>>()
                            .ok()?;
                        if values.is_empty() || !values.len().is_multiple_of(2) {
                            return None;
                        }

                        let pairs = values
                            .as_chunks::<2>()
                            .0
                            .iter()
                            .map(|c| (c[0], c[1]))
                            .collect::<Vec<_>>();
                        Some(Command::Terminal(Message::SetFrameBufferRange(pairs)))
                    }),
                ),
                "memory_viewer_open" => single_word(
                    arrays.clone(),
                    Box::new(|word| {
                        Some(Command::Terminal(Message::OpenMemoryViewer {
                            scope: ScopeRef::from_hierarchy_string(word),
                            name: Some(word.to_string()),
                        }))
                    }),
                ),
                "dump_tree" => Some(Command::Terminal(Message::DumpTree)),
                "group_marked" => optional_single_word(
                    vec![],
                    Box::new(|name| {
                        let trimmed = name.trim();
                        Some(Command::Terminal(Message::GroupNew {
                            name: (!trimmed.is_empty()).then_some(trimmed.to_owned()),
                            before: None,
                            items: None,
                        }))
                    }),
                ),
                "group_dissolve" => Some(Command::Terminal(Message::GroupDissolve(None))),
                "group_fold_recursive" => {
                    Some(Command::Terminal(Message::GroupFoldRecursive(None)))
                }
                "group_unfold_recursive" => {
                    Some(Command::Terminal(Message::GroupUnfoldRecursive(None)))
                }
                "group_fold_all" => Some(Command::Terminal(Message::GroupFoldAll)),
                "group_unfold_all" => Some(Command::Terminal(Message::GroupUnfoldAll)),
                "show_controls" => Some(Command::Terminal(Message::SetKeyHelpVisible(true))),
                "show_mouse_gestures" => {
                    Some(Command::Terminal(Message::SetGestureHelpVisible(true)))
                }
                "show_quick_start" => Some(Command::Terminal(Message::SetQuickStartVisible(true))),
                #[cfg(feature = "performance_plot")]
                "show_performance" => optional_single_word(
                    vec![],
                    Box::new(|word| {
                        if word == "redraw" {
                            Some(Command::Terminal(Message::Batch(vec![
                                Message::SetPerformanceVisible(true),
                                Message::SetContinuousRedraw(true),
                            ])))
                        } else {
                            Some(Command::Terminal(Message::SetPerformanceVisible(true)))
                        }
                    }),
                ),
                "cursor_set" => {
                    let timescale_for_cursor = timescale.clone();
                    single_word(
                        vec![],
                        Box::new(move |time_str| {
                            let time = if let Some(ts) = &timescale_for_cursor {
                                crate::time::parse_time_string_to_ticks(time_str, ts)?
                            } else {
                                time_str.parse().ok()?
                            };
                            Some(Command::Terminal(Message::Batch(vec![
                                Message::CursorSet(time),
                                Message::GoToCursorIfNotInView,
                            ])))
                        }),
                    )
                }
                "goto_time" => {
                    let timescale_for_goto = timescale.clone();
                    single_word(
                        vec![],
                        Box::new(move |time_str| {
                            let time = if let Some(ts) = &timescale_for_goto {
                                crate::time::parse_time_string_to_ticks(time_str, ts)?
                            } else {
                                time_str.parse().ok()?
                            };
                            Some(Command::Terminal(Message::GoToTime(Some(time), 0)))
                        }),
                    )
                }
                "marker_set" => Some(Command::NonTerminal(
                    ParamGreed::Custom(&separate_at_space),
                    // FIXME use once fzcmd does not enforce suggestion match, as of now we couldn't add a marker (except the first)
                    // marker_suggestions(&markers),
                    vec![],
                    Box::new(move |name, _| {
                        let name = name.to_owned();

                        Some(Command::NonTerminal(
                            ParamGreed::Word,
                            vec![],
                            Box::new(move |time_str, _| {
                                let time = time_str.parse().ok()?;
                                Some(Command::Terminal(Message::ResolveMarkerSet {
                                    name: name.clone(),
                                    time,
                                }))
                            }),
                        ))
                    }),
                )),
                "marker_set_at" => {
                    let timescale_for_marker_set_at = timescale.clone();
                    Some(Command::NonTerminal(
                        ParamGreed::Rest,
                        vec![],
                        Box::new(move |query, _| {
                            let parts = query.split_whitespace().collect_vec();
                            if parts.len() < 2 {
                                return None;
                            }

                            let (time_str, marker_ref) = if parts.len() >= 3 {
                                let combined = format!("{}{}", parts[0], parts[1]);
                                if let Some(ts) = &timescale_for_marker_set_at {
                                    if crate::time::parse_time_string_to_ticks(&combined, ts)
                                        .is_some()
                                    {
                                        (combined, parts[2..].join(" "))
                                    } else {
                                        (parts[0].to_string(), parts[1..].join(" "))
                                    }
                                } else {
                                    (parts[0].to_string(), parts[1..].join(" "))
                                }
                            } else {
                                (parts[0].to_string(), parts[1].to_string())
                            };

                            let time = if let Some(ts) = &timescale_for_marker_set_at {
                                crate::time::parse_time_string_to_ticks(&time_str, ts)?
                            } else {
                                time_str.parse().ok()?
                            };

                            let marker_id = parse_marker(&marker_ref, &markers);
                            match marker_id {
                                Some(id) => {
                                    Some(Command::Terminal(Message::SetMarker { id, time }))
                                }
                                None => Some(Command::Terminal(Message::AddMarker {
                                    time,
                                    name: Some(marker_ref),
                                    move_focus: true,
                                })),
                            }
                        }),
                    ))
                }
                "marker_remove" => Some(Command::NonTerminal(
                    ParamGreed::Rest,
                    marker_suggestions(&markers),
                    Box::new(move |name, _| {
                        Some(Command::Terminal(Message::ResolveMarkerRemove(
                            name.to_owned(),
                        )))
                    }),
                )),
                "show_marker_window" => {
                    Some(Command::Terminal(Message::SetCursorWindowVisible(true)))
                }
                "show_logs" => Some(Command::Terminal(Message::SetLogsVisible(true))),
                "save_state" => Some(Command::Terminal(Message::SaveStateFile(
                    state_file.clone(),
                ))),
                "save_state_as" => single_word(
                    vec![],
                    Box::new(|word| {
                        Some(Command::Terminal(Message::SaveStateFile(Some(
                            Utf8PathBuf::from(word),
                        ))))
                    }),
                ),
                "load_state" => single_word(
                    vec![],
                    Box::new(|word| {
                        Some(Command::Terminal(Message::LoadStateFile(Some(
                            Utf8PathBuf::from(word),
                        ))))
                    }),
                ),
                "viewport_add" => Some(Command::Terminal(Message::AddViewport)),
                "viewport_remove" => Some(Command::Terminal(Message::RemoveViewport)),
                "viewport_set_active" => single_word(
                    viewport_indices.clone(),
                    Box::new(|word| {
                        let idx = word.parse::<usize>().ok()?;
                        Some(Command::Terminal(Message::SetActiveViewport(idx)))
                    }),
                ),
                "pause_simulation" => Some(Command::Terminal(Message::PauseSimulation)),
                "unpause_simulation" => Some(Command::Terminal(Message::UnpauseSimulation)),
                "undo" => Some(Command::Terminal(Message::Undo(1))),
                "redo" => Some(Command::Terminal(Message::Redo(1))),
                "wcp_server_start" => Some(Command::Terminal(Message::StartWcpServer {
                    address: None,
                    initiate: false,
                })),
                "wcp_server_stop" => Some(Command::Terminal(Message::StopWcpServer)),
                "exit" => Some(Command::Terminal(Message::Exit)),
                _ => None,
            }
        }),
    )
}
