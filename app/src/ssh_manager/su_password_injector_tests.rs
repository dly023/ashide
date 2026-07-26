use super::is_su_to_root;
use super::SU_ROOT_CMD_REGEX;
use crate::ssh_manager::password_prompt::bytes_look_like_password_prompt;

fn su_matches(input: &str) -> bool {
    SU_ROOT_CMD_REGEX.is_match(input.as_bytes())
}

#[test]
fn su_root_matches_common_variants() {
    // 最基本
    assert!(su_matches("su"));
    assert!(su_matches("su\n"));
    // 不带用户名的快捷形式(默认 root)
    assert!(su_matches("su -"));
    assert!(su_matches("su -l"));
    assert!(su_matches("su --login"));
    // 显式 root
    assert!(su_matches("su root"));
    assert!(su_matches("su - root"));
    assert!(su_matches("su -l root"));
    assert!(su_matches("su --login root"));
    // sudo su(\bsu 仍能命中)
    assert!(su_matches("sudo su"));
}

#[test]
fn su_to_other_user_does_not_match() {
    // 切到非 root 用户不应触发
    assert!(!su_matches("su lg"));
    assert!(!su_matches("su - lg"));
    assert!(!su_matches("su -l lg"));
    assert!(!su_matches("su --login lg"));
    assert!(!su_matches("su admin"));
}

#[test]
fn su_in_middle_of_other_command_does_not_match() {
    // su 不在行尾不应触发
    assert!(!su_matches("susan"));
    assert!(!su_matches("issue"));
    // grep su file 这种命令,行尾不是 su 也不是 su root 模式
    assert!(!su_matches("grep su /etc/passwd"));
}

#[test]
fn is_su_to_root_detects_in_buffer() {
    let buf = b"user@host:~$ su root\r\nPassword: ";
    assert!(is_su_to_root(buf));

    let buf = b"user@host:~$ su lg\r\nPassword: ";
    assert!(!is_su_to_root(buf));
}

#[test]
fn full_pipeline_su_root_with_password_prompt() {
    // 模拟完整 PTY 序列:用户输入 `su -`,回显后出现密码提示
    let buf = b"alice@kylin:~$ su -\r\n\xe5\xaf\x86\xe7\xa0\x81\xef\xbc\x9a";
    assert!(bytes_look_like_password_prompt(buf));
    assert!(is_su_to_root(buf));
}
