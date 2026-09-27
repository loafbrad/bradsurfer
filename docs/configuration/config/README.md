# Config file

This page documents the user configuration file loaded by Surfer on native builds.
You only need to specify the settings you want to change; any omitted setting keeps its default value.

The complete default configuration lives in [default_config.toml](https://gitlab.com/surfer-project/surfer/-/blob/main/default_config.toml?ref_type=heads).

## Example

```toml
theme = "dark+"
default_variable_name_type = "Global"
snap_distance = 10

[default_time_format]
format = "SI"
show_space = true
show_unit = true

[layout]
show_ticks = false
hierarchy_style = "Tree"
waveforms_text_size = 12.0

[layout.toolbar.row]
menu = 0
time = 1

[layout.toolbar.visibility]
menu = false
cxxrtl = false

[behavior]
arrow_key_bindings = "Scroll"
primary_button_drag_behavior = "Measure"
```

## Load order

On native builds, configuration is loaded in this order, with later sources overriding earlier ones:

1. Built-in defaults from `default_config.toml`
2. The per-user `config.toml` in Surfer's configuration directory
3. Deprecated `surfer.toml` in the current working directory, if present
4. Any `.surfer/config.toml` files found from the filesystem root down to the current directory
5. Environment variables with the `SURFER` prefix

## Top-level settings

| Key | Default | Values | Description |
| --- | --- | --- | --- |
| `default_variable_name_type` | `"Unique"` | `Local`, `Unique`, `Global` | Default signal name display style. |
| `default_clock_highlight_type` | `"Line"` | `Line`, `Cycle`, `None` | Default clock highlighting mode. |
| `snap_distance` | `6` | non-negative number | Cursor snap distance in pixels. |
| `theme` | `""` | theme name | Theme to load. Leave empty to use the built-in default theme. |
| `undo_stack_size` | `50` | integer | Maximum number of undo steps to keep. |
| `autoreload_files` | `"Ask"` | `Always`, `Never`, `Ask` | What to do when loaded waveform files change on disk. |
| `autoload_sibling_state_files` | `"Ask"` | `Always`, `Never`, `Ask` | Whether matching state files should be loaded automatically. |
| `animation_time` | `0.1` | non-negative number | Duration of UI animations in seconds. |
| `animation_enabled` | `true` | boolean | Enable or disable UI animations entirely. |
| `show_divider_text` | `false` | boolean | Show divider labels inline in the waveform area. |
| `max_url_length` | `65534` | integer | Maximum URL length used for remote connections. Useful when a proxy enforces a limit. |

The remaining top-level keys are tables documented below: `default_time_format`, `layout`, `gesture`, `behavior`, `wcp`, `plugin`, `server`, and `shortcuts`.

## `[default_time_format]`

Controls how time values are rendered in the UI.

| Key | Default | Values | Description |
| --- | --- | --- | --- |
| `format` | `"No"` | `No`, `Locale`, `SI` | Numeric formatting style. `Locale` uses the current locale. `SI` groups digits using SI-style spacing. |
| `show_space` | `true` | boolean | Insert a space between the numeric part and the unit. |
| `show_unit` | `true` | boolean | Show the time unit suffix. |

## `[layout]`

Controls the initial UI layout and waveform rendering behavior.

| Key | Default | Values | Description |
| --- | --- | --- | --- |
| `show_hierarchy` | `true` | boolean | Show the hierarchy panel. |
| `show_menu` | `true` | boolean | Show the menu bar. |
| `show_toolbar` | `true` | boolean | Show the toolbar. |
| `show_ticks` | `true` | boolean | Show vertical tick lines in the waveform area. |
| `show_tooltip` | `true` | boolean | Show tooltips for variables. |
| `show_scope_tooltip` | `false` | boolean | Show tooltips for scopes. |
| `show_overview` | `true` | boolean | Show the overview panel. |
| `show_statusbar` | `true` | boolean | Show the status bar. |
| `show_variable_indices` | `true` | boolean | Show signal indices in the variable list when available. |
| `show_variable_direction` | `true` | boolean | Show direction icons or indicators for variables. |
| `show_default_timeline` | `true` | boolean | Add a timeline row by default. |
| `show_empty_scopes` | `false` | boolean | Show scopes that contain no visible items. |
| `show_hierarchy_icons` | `false` | boolean | Show scope and variable icons in the hierarchy. |
| `parameter_display_location` | `"Scopes"` | `Variables`, `Scopes`, `Tooltips`, `None` | Where parameter values are displayed in the hierarchy UI. |
| `window_width` | `1920` | integer | Initial window width in pixels. |
| `window_height` | `1080` | integer | Initial window height in pixels. |
| `window_x_position` | `0` | integer | Initial window x-position in pixels. |
| `window_y_position` | `0` | integer | Initial window y-position in pixels. |
| `align_names_right` | `false` | boolean | Right-align names in the item list. |
| `hierarchy_style` | `"Separate"` | `Separate`, `Tree`, `Variables` | Layout style used for the hierarchy and variable list. |
| `waveforms_text_size` | `11.0` | non-negative number | Text size for waveform values, in points. |
| `waveforms_line_height` | `16.0` | non-negative number | Base line height for waveforms, in points. |
| `waveforms_gap` | `2.5` | non-negative number | Vertical gap above and below waveform traces. Basically, how far the background is drawn. |
| `waveforms_line_height_multiples` | `[1, 2, 4, 8, 16]` | list of non-negative numbers | Available line-height multipliers for taller rows. |
| `analog_waveform_multiplier` | `4` | non-negative number | Default height multiplier applied when a variable is switched from digital to analog rendering. |
| `transactions_line_height` | `30.0` | non-negative number | Line height for transaction streams. |
| `zoom_factors` | `[0.5, 0.75, 0.9, 1.0, 1.1, 1.25, 1.5, 2.0, 2.5]` | list of non-negative numbers | Available UI zoom factors. |
| `default_zoom_factor` | `1.0` | non-negative number | Initial UI zoom factor. |
| `focus_highlight` | `"Off"` | `Off`, `Background`, `LineWidth`, `BrightnessShift`, `LineWidthAndBrightnessShift` | How to highlight the focused waveform. |
| `move_focus_on_inserted_marker` | `true` | boolean | Move focus to newly inserted markers. |
| `fill_high_values` | `true` | boolean | Fill the high state in boolean waveforms. |
| `trace_style` | `"Default"` | `Default`, `Dinotrace`, `Zero` | Digital waveform trace style. `Dinotrace` draws no upper line and a bold lower line for all-zero vectors, and a bold upper line for all-one vectors. `Zero` draws all-zero vectors without the upper line. |
| `transition_value` | `"Next"` | `Previous`, `Next`, `Both` | Which value to show when the cursor is exactly on a transition. |
| `draw_vector_unknowns_as_line` | `false` | boolean | Draw vector unknowns as a line instead of a "box". |
| `enable_time_offset` | `false` | boolean | If true, only draw from the first time where a waveform has values and not from 0. |

The `layout` table also contains toolbar-group subtables under `layout.toolbar`, documented below: `layout.toolbar.row` and `layout.toolbar.visibility`.

### `[layout.toolbar.row]`

Controls the default row assignment for each toolbar group. Each key is a toolbar group id and each value is a row number stored as an unsigned 8-bit integer.

| Key | Default | Values | Description |
| --- | --- | --- | --- |
| `menu` | `0` | integer from `0` to `255` | Row for the menu group. |
| `files` | `0` | integer from `0` to `255` | Row for the file actions group. |
| `copy` | `0` | integer from `0` to `255` | Row for the copy group. |
| `zoom` | `0` | integer from `0` to `255` | Row for the zoom group. |
| `navigation` | `0` | integer from `0` to `255` | Row for the navigation group. |
| `transitions` | `0` | integer from `0` to `255` | Row for the transition-jump group. |
| `add_items` | `0` | integer from `0` to `255` | Row for the add-items group. |
| `viewports` | `0` | integer from `0` to `255` | Row for the viewport group. |
| `undo` | `0` | integer from `0` to `255` | Row for the undo/redo group. |
| `cxxrtl` | `0` | integer from `0` to `255` | Row for the CXXRTL simulation controls group. |
| `time` | `0` | integer from `0` to `255` | Row for the time-input group. |
| `annotations` | `0` | integer from `0` to `255` | Row for the annotations group. |
| `edit` | `0` | integer from `0` to `255` | Row for the signal editing group. |

### `[layout.toolbar.visibility]`

Controls the default visibility of each toolbar group. These values are only used as defaults; once a state file stores toolbar-group visibility, the state file wins.

| Key | Default | Values | Description |
| --- | --- | --- | --- |
| `menu` | `true` | boolean | Show the menu group by default. |
| `files` | `true` | boolean | Show the file actions group by default. |
| `copy` | `true` | boolean | Show the copy group by default. |
| `zoom` | `true` | boolean | Show the zoom group by default. |
| `navigation` | `true` | boolean | Show the navigation group by default. |
| `transitions` | `true` | boolean | Show the transition-jump group by default. |
| `add_items` | `true` | boolean | Show the add-items group by default. |
| `viewports` | `true` | boolean | Show the viewport group by default. |
| `undo` | `true` | boolean | Show the undo/redo group by default. |
| `cxxrtl` | `true` | boolean | Show the CXXRTL simulation controls group by default. |
| `time` | `true` | boolean | Show the time-input group by default. |
| `annotations` | `true` | boolean | Show the annotations group by default. |
| `edit` | `true` | boolean | Show the signal editing group by default. |

Note that some of the groups are not shown if no wave is loaded and in some other situations. For example, the menu group is never shown when the regular menu is shown.

## `[gesture]`

Controls the radial mouse-gesture overlay shown when using gesture mode.

| Key | Default | Values | Description |
| --- | --- | --- | --- |
| `size` | `300` | non-negative number | Size of the gesture help overlay. |
| `deadzone` | `20` | non-negative number | Minimum squared drag distance before a gesture action is triggered. |
| `background_radius` | `1.35` | non-negative number | Background circle radius as a factor of `size / 2`. |
| `background_gamma` | `0.75` | number between `0` and `1` | Background opacity factor. Lower values are more opaque. |

### `[gesture.mapping]`

Maps each drag direction to a gesture action.

Supported actions are `Cancel`, `ZoomIn`, `ZoomOut`, `ZoomToFit`, `GoToEnd`, and `GoToStart`.

| Direction | Default |
| --- | --- |
| `north` | `"Cancel"` |
| `northeast` | `"ZoomOut"` |
| `east` | `"ZoomIn"` |
| `southeast` | `"GoToEnd"` |
| `south` | `"Cancel"` |
| `southwest` | `"GoToStart"` |
| `west` | `"ZoomIn"` |
| `northwest` | `"ZoomToFit"` |

## `[behavior]`

Controls a small set of interaction defaults.

| Key | Default | Values | Description |
| --- | --- | --- | --- |
| `keep_during_reload` | `true` | boolean | Keep variables/items when they are unavailable after a reload. |
| `file_history_size` | `10` | integer | Maximum number of entries to keep in the recent file history. |
| `arrow_key_bindings` | `"Edge"` | `Edge`, `Scroll` | Make left/right arrow keys jump between edges or scroll the viewport. |
| `primary_button_drag_behavior` | `"Cursor"` | `Cursor`, `Measure` | Default behavior for primary-button dragging. Holding Shift temporarily selects the other mode. |

## `[wcp]`

Waveform Control Protocol server settings.

| Key | Default | Values | Description |
| --- | --- | --- | --- |
| `autostart` | `false` | boolean | Start the WCP server automatically on launch. |
| `address` | `"127.0.0.1:54321"` | `host:port` string | Bind address for the WCP server. |

## `[plugin]`

Settings for waveform translator plugins.

| Key | Default | Values | Description |
| --- | --- | --- | --- |
| `max_memory_mib` | `10` | positive integer | Maximum memory budget in MiB available to each WASM translator plugin. Increase this if a plugin fails due to memory limits. |

## `[server]`

Settings for Surver's HTTP server.

| Key | Default | Values | Description |
| --- | --- | --- | --- |
| `bind_address` | `"127.0.0.1"` | host or IP string | Address to bind the server to. |
| `port` | `8911` | integer | TCP port to listen on. |

## `[shortcuts]`

The `shortcuts` table maps an action name to a list of key chords. Each value is an array of strings such as `"Command+O"` or `"PageDown"`, where each value in the list is one shortcut, not a sequence. Hence, each action can have multiple alternative shortcuts.

`Command` corresponds to ⌘ on Mac and `Ctrl` on all other platforms. For a list of key names, see [Key](https://docs.rs/egui/latest/egui/enum.Key.html).

The default configuration defines these actions:

| Action | Default binding |
| --- | --- |
| `delete_selected` | `Delete`, `X` |
| `divider_add` | `D` |
| `focus_variable_name_filter` | `V` |
| `go_to_time` | `Command+G` |
| `goto_bottom` | `End` |
| `goto_end` | `E` |
| `goto_start` | `S` |
| `goto_top` | `Home` |
| `group_new` | `G` |
| `item_focus` | `F` |
| `marker_add` | `M` |
| `open_file` | `Command+O` |
| `redo` | `Command+Shift+Z`, `Command+Y` |
| `reload_waveform` | `R` |
| `rename_item` | `F2` |
| `save_state_file` | `Command+S` |
| `scroll_down` | `PageDown` |
| `scroll_up` | `PageUp` |
| `select_all` | `Command+A` |
| `select_toggle` | `A` |
| `show_command_prompt` | `Space` |
| `switch_file` | `Command+Shift+O` |
| `toggle_menu` | `Alt+M` |
| `toggle_side_panel` | `B` |
| `toggle_toolbar` | `T` |
| `ui_zoom_in` | `Command+Plus` |
| `ui_zoom_out` | `Command+Minus` |
| `undo` | `Command+Z`, `U` |
| `zoom_in` | `Plus`, `Equals` |
| `zoom_out` | `Minus` |
| `zoom_to_cursor` | `Shift+Z` |
| `zoom_to_fit` | `Shift+F` |

## Notes

- Floating-point values that are documented as non-negative are clamped to `0` if a negative value is provided.
- Values documented as being between `0` and `1` are clamped to that range.
- Theme files are documented separately in the configuration section's themes page.
