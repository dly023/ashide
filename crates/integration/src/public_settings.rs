use warp::terminal::block_list_viewport::InputMode;

/// 返回设置了终端输入布局的公开 `settings.toml` 文档。
pub fn input_mode(input_mode: InputMode) -> String {
    let value = match input_mode {
        InputMode::PinnedToBottom => "pinned_to_bottom",
        InputMode::PinnedToTop => "pinned_to_top",
        InputMode::Waterfall => "waterfall",
    };
    format!("[appearance.input]\ninput_mode = \"{value}\"\n")
}
