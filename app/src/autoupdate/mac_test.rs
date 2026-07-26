use super::parse_code_signature_team_identifier;

#[test]
fn test_parse_code_signature_team_identifier() {
    assert_eq!(
        parse_code_signature_team_identifier(
            b"Executable=/Applications/Ashide.app/Contents/MacOS/ashide\nTeamIdentifier=ABCDEFGHIJ\n"
        )
        .unwrap(),
        "ABCDEFGHIJ"
    );
}

#[test]
fn test_parse_code_signature_team_identifier_rejects_missing_or_adhoc_identity() {
    assert!(
        parse_code_signature_team_identifier(b"Signature=adhoc\nTeamIdentifier=not set\n")
            .unwrap_err()
            .to_string()
            .contains("not a valid target-owned Apple Team ID")
    );
    assert!(parse_code_signature_team_identifier(b"Signature=adhoc\n")
        .unwrap_err()
        .to_string()
        .contains("missing TeamIdentifier"));
}

#[test]
fn test_parse_code_signature_team_identifier_rejects_malformed_identity() {
    for invalid in ["abcdefghij", "ABCDEFGHI", "ABCDEFGHIJK", "ABCDE-FGHI"] {
        let metadata = format!("TeamIdentifier={invalid}\n");
        assert!(parse_code_signature_team_identifier(metadata.as_bytes()).is_err());
    }
}
