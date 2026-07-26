use super::bytes_look_like_password_prompt;

fn matches(input: &str) -> bool {
    bytes_look_like_password_prompt(input.as_bytes())
}

#[test]
fn matches_typical_password_prompt() {
    assert!(matches("user@host's password: "));
    assert!(matches("Password:"));
    assert!(matches("password: \r\n"));
}

#[test]
fn matches_sudo_password_prompt() {
    assert!(matches("[sudo] password for alice: "));
}

#[test]
fn matches_passphrase_prompt() {
    assert!(matches("Enter passphrase for key '/home/u/.ssh/id_rsa': "));
}

#[test]
fn matches_cjk_and_fullwidth_colon() {
    // 全角冒号(中文输入法)
    assert!(matches("密码:"));
    assert!(matches("密码："));
    // 银河麒麟 V10 无冒号特例
    assert!(matches("输入密码"));
    assert!(matches("输入密码 "));
}

#[test]
fn does_not_match_motd_with_password_word() {
    assert!(!matches("Welcome! Please change your password soon.\n# "));
    assert!(!matches(
        "Last login: Mon Jan 1 password rotated yesterday\n"
    ));
    // 含 'password' / '密码' 但非真正提示的输出,不能假阳性
    assert!(!matches("Your password has expired"));
    assert!(!matches("Bad password, try again"));
    assert!(!matches("password changed successfully"));
    assert!(!matches("New password for root"));
    assert!(!matches("您的密码已过期"));
}

#[test]
fn does_not_match_no_colon() {
    assert!(!matches("password\n"));
    assert!(!matches("Enter password please\n"));
}
