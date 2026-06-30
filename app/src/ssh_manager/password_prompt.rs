use lazy_static::lazy_static;
use regex::bytes::Regex;

/// 单一权威的密码提示符正则。严格匹配两类:
/// 1. `password` / `passphrase` / `密码` 行尾带半角冒号 `:` 或全角冒号 `：`
/// 2. 银河麒麟 V10 的无冒号 `输入密码`
///
/// 冒号(或麒麟特例)是必需的:`Your password has expired`、
/// `Last login: ... password rotated` 这类含 "password"/"密码" 但非真实提示的
/// 行尾不会假阳性。OneKey 凭据菜单(terminal/view.rs)与 su root 监听
/// (su_password_injector.rs)都共用此函数,避免正则分歧。
const PASSWORD_PROMPT_PATTERN: &str =
    r"(?im)(?:(?:password|passphrase|密码)[^\n]*(?::|：)\s*$|输入密码\s*$)";

lazy_static! {
    static ref PASSWORD_PROMPT_REGEX: Regex =
        Regex::new(PASSWORD_PROMPT_PATTERN).expect("password prompt regex must compile");
}

pub fn bytes_look_like_password_prompt(bytes: &[u8]) -> bool {
    PASSWORD_PROMPT_REGEX.is_match(bytes)
}

#[cfg(test)]
#[path = "password_prompt_tests.rs"]
mod tests;
