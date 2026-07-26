use std::path::Path;
use std::process::Command;

#[test]
fn macos_release_shell_scripts_parse_with_declared_shell() {
    let repository_root = Path::new(file!())
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .expect("release contract must live under script/tests");

    for relative_path in [
        "script/macos/bundle",
        "script/macos/resolve_signing_mode",
        "script/macos/verify_public_distribution",
        "script/make_release_artifacts",
        "script/tests/macos_public_distribution_contract.sh",
    ] {
        let script = repository_root.join(relative_path);
        let output = Command::new("bash")
            .arg("-n")
            .arg(&script)
            .output()
            .unwrap_or_else(|error| panic!("failed to parse {}: {error}", script.display()));
        assert!(
            output.status.success(),
            "{} failed bash -n:\n{}",
            script.display(),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn macos_unsigned_release_bundle_is_ad_hoc_sealed_and_verified() {
    let workflow = include_str!("../../.github/workflows/release.yml");
    let macos_bundle = include_str!("../macos/bundle");
    let artifact_script = include_str!("../make_release_artifacts");

    assert!(workflow
        .contains("building an ad-hoc sealed app without Developer ID signing or notarization"));
    assert!(
        macos_bundle.contains("codesign --force --deep --sign - --timestamp=none \"$app_bundle\"")
    );
    assert!(macos_bundle.contains("codesign --verify --deep --strict --verbose=4 \"$app_bundle\""));
    assert!(
        artifact_script.contains("codesign --verify --deep --strict --verbose=4 \"$APP_BUNDLE\"")
    );

    let seal_call = macos_bundle
        .rfind("\n  seal_and_verify_app_bundle\n")
        .expect("app bundle seal postcondition call must exist");
    let dmg_step = macos_bundle
        .find("## Step 4: Create DMG ##")
        .expect("DMG assembly step must exist");
    assert!(seal_call < dmg_step);

    let packaging_verify = artifact_script
        .find("codesign --verify --deep --strict --verbose=4 \"$APP_BUNDLE\"")
        .expect("release packaging must verify the complete app bundle seal");
    let packaging_start = artifact_script
        .find("if [[ ! -f \"$DMG_FILE\" ]]")
        .expect("release packaging must require the DMG");
    assert!(packaging_verify < packaging_start);
}

#[test]
fn macos_release_publishes_direct_verified_dmg_instead_of_zip() {
    let workflow = include_str!("../../.github/workflows/release.yml");
    let artifact_script = include_str!("../make_release_artifacts");

    assert!(artifact_script.contains("OUTPUT_PATH=\"$ARTIFACTS_DIR/$NAME.dmg\""));
    assert!(artifact_script.contains("hdiutil verify \"$DMG_FILE\""));
    assert!(artifact_script.contains("ditto \"$DMG_FILE\" \"$OUTPUT_PATH\""));
    assert!(!artifact_script.contains("ditto -c -k --keepParent \"$DMG_FILE\""));
    assert!(!artifact_script.contains("elif [[ -d \"$APP_BUNDLE\" ]]"));

    assert!(workflow.contains("target/release-artifacts/Ashide-macos.dmg"));
    assert!(workflow.contains("target/release-artifacts/Ashide-macos.dmg.sha256"));
    assert!(!workflow.contains("target/release-artifacts/Ashide-macos.zip"));
}

#[test]
fn macos_release_format_migration_removes_stale_zip_and_checksum() {
    let artifact_script = include_str!("../make_release_artifacts");

    assert!(artifact_script.contains("OBSOLETE_OUTPUT_PATH=\"$ARTIFACTS_DIR/$NAME.zip\""));
    assert!(artifact_script
        .contains("rm -f \"$OBSOLETE_OUTPUT_PATH\" \"$OBSOLETE_OUTPUT_PATH.sha256\""));
    assert!(artifact_script.contains("OBSOLETE_ARTIFACT_BASENAME="));
    assert!(artifact_script.contains("$2 != current && $2 != obsolete"));
}

#[test]
fn macos_release_keeps_direct_dmg_available_with_optional_complete_signing_configuration() {
    let workflow = include_str!("../../.github/workflows/release.yml");
    let macos_bundle = include_str!("../macos/bundle");
    let signing_resolver = include_str!("../macos/resolve_signing_mode");
    let release_entitlements = include_str!("../Entitlements.plist");
    let runtime_macos = include_str!("../../crates/warp_core/src/macos.rs");
    let autoupdate_macos = include_str!("../../app/src/autoupdate/mac.rs");

    for secret in [
        "ASHIDE_DEVELOPER_ID_CERT",
        "ASHIDE_DEVELOPER_ID_CERT_PASSWORD",
        "ASHIDE_CODESIGN_KEYCHAIN_PASSWORD",
        "ASHIDE_NOTARIZATION_APPLE_ID",
        "ASHIDE_NOTARIZATION_PASSWORD",
        "ASHIDE_NOTARIZATION_TEAM_ID",
    ] {
        assert!(workflow.contains(secret), "workflow must receive {secret}");
        assert!(
            macos_bundle.contains(secret),
            "bundle must validate {secret}"
        );
        assert!(
            signing_resolver.contains(secret),
            "shared resolver must own {secret}"
        );
    }

    assert!(workflow.contains("PUBLISH_RELEASE_ASSETS"));
    assert!(!workflow.contains("REQUIRE_PUBLIC_MACOS_DISTRIBUTION"));
    assert!(workflow.contains(r#"signing_mode="$(./script/macos/resolve_signing_mode)""#));
    assert!(!workflow.contains("resolve_signing_mode public"));
    assert!(!workflow.contains("resolve_signing_mode development"));
    assert!(!workflow.contains("macos_signing_secret_count"));
    assert!(workflow.contains("bundle_args+=(--distribution public --read-passwords-from-env)"));
    assert!(workflow.contains("bundle_args+=(--distribution development --nosign)"));
    assert!(workflow.contains("artifact_args+=(--distribution development)"));
    assert!(signing_resolver.contains(r#""$configured_count" -ne 0 && "$configured_count" -ne 6"#));
    assert!(!signing_resolver.contains("Public macOS releases require all six"));

    assert!(!macos_bundle.contains("APPLE_TEAM_ID=\""));
    assert!(!macos_bundle.contains("-s \"$APPLE_TEAM_ID\""));
    assert!(!release_entitlements.contains("com.apple.security.application-groups"));
    assert!(!runtime_macos.contains("APPLE_TEAM_ID"));
    assert!(autoupdate_macos.contains("code_signature_team_identifier(&current_bundle_path)"));
    assert!(autoupdate_macos.contains("expected_team_identifier"));
    assert!(macos_bundle.contains("Developer ID Application:"));
    assert!(macos_bundle.contains("DEVELOPER_ID_IDENTITY_COUNT"));
    assert!(macos_bundle.contains("$ASHIDE_NOTARIZATION_TEAM_ID"));
    assert!(macos_bundle.contains("script/macos/resolve_signing_mode"));

    let bundle_preflight = macos_bundle
        .find("Public macOS distributions require explicit Developer ID")
        .expect("public distribution preflight must exist");
    let build_step = macos_bundle
        .find("## Step 1: Build the app ##")
        .expect("bundle build step must exist");
    assert!(bundle_preflight < build_step);
}

#[test]
fn macos_signed_release_requires_developer_id_notarization_and_gatekeeper_postconditions() {
    let workflow = include_str!("../../.github/workflows/release.yml");
    let macos_bundle = include_str!("../macos/bundle");
    let artifact_script = include_str!("../make_release_artifacts");
    let public_verifier = include_str!("../macos/verify_public_distribution");

    assert!(workflow.contains("--distribution public"));
    assert!(macos_bundle.contains("script/macos/verify_public_distribution"));
    assert!(artifact_script.contains("script/macos/verify_public_distribution"));

    assert!(public_verifier.contains("Authority=Developer ID Application:"));
    assert!(public_verifier.contains("TeamIdentifier=$EXPECTED_TEAM_ID"));
    assert!(public_verifier.contains("flags=.*runtime"));
    assert!(public_verifier.contains("codesign --verify --deep --strict"));
    assert!(public_verifier.contains("xcrun stapler validate"));
    assert!(public_verifier.contains("spctl --assess --type execute"));
    assert!(
        public_verifier.contains("spctl --assess --type open --context context:primary-signature")
    );

    let bundle_verify = macos_bundle
        .find("script/macos/verify_public_distribution")
        .expect("bundle must run the shared public distribution verifier");
    let bundle_copy = macos_bundle
        .find("## Step 6: Copy and output artifacts ##")
        .expect("bundle copy step must exist");
    assert!(bundle_verify < bundle_copy);

    let packaging_verify = artifact_script
        .find("script/macos/verify_public_distribution")
        .expect("packager must run the shared public distribution verifier");
    let packaging_copy = artifact_script
        .find("ditto \"$DMG_FILE\" \"$OUTPUT_PATH\"")
        .expect("packager copy step must exist");
    assert!(packaging_verify < packaging_copy);
}

#[test]
fn macos_release_builds_app_and_helpers_in_independent_jobs() {
    let workflow = include_str!("../../.github/workflows/release.yml");

    let app_start = workflow
        .find("\n  build-macos-app:\n")
        .expect("macOS App/DMG producer job must exist");
    let helpers_start = workflow
        .find("\n  build-macos-helpers:\n")
        .expect("macOS helper producer job must exist");
    let linux_start = workflow
        .find("\n  build-linux-cli:\n")
        .expect("Linux producer job must exist");
    let upload_start = workflow
        .find("\n  upload-release:\n")
        .expect("release merge job must exist");

    assert!(app_start < helpers_start);
    assert!(helpers_start < linux_start);

    let app_job = &workflow[app_start..helpers_start];
    let helper_job = &workflow[helpers_start..linux_start];
    let upload_job = &workflow[upload_start..];

    assert!(app_job.contains("needs: validate-release-identity"));
    assert!(helper_job.contains("needs: validate-release-identity"));
    assert!(app_job.contains("timeout-minutes:"));
    assert!(helper_job.contains("timeout-minutes:"));

    assert!(app_job.contains("Build macOS app bundle"));
    assert!(app_job.contains("Package macOS artifact"));
    assert!(app_job.contains("name: release-macos-app"));
    assert!(app_job.contains("target/release-artifacts/Ashide-macos.dmg"));
    assert!(!app_job.contains("make_release_helper_artifacts"));
    assert!(!app_job.contains("ashide-macos-x86_64.tar.gz"));

    assert!(helper_job.contains("make_release_helper_artifacts"));
    assert!(helper_job.contains("arch: x86_64"));
    assert!(helper_job.contains("archive: ashide-macos-x86_64.tar.gz"));
    assert!(helper_job.contains("arch: aarch64"));
    assert!(helper_job.contains("archive: ashide-macos-aarch64.tar.gz"));
    assert!(helper_job.contains(r#"--arch "${{ matrix.arch }}""#));
    assert!(helper_job.contains("name: release-macos-helper-${{ matrix.arch }}"));
    assert!(helper_job.contains("timeout-minutes: 360"));
    assert!(helper_job.contains("path: target/release-artifacts/${{ matrix.archive }}"));
    assert!(!helper_job.contains("Build macOS app bundle"));
    assert!(!helper_job.contains("Ashide-macos.dmg"));

    assert!(upload_job.contains("- build-macos-app"));
    assert!(upload_job.contains("- build-macos-helpers"));
    assert!(!upload_job.contains("- build-macos\n"));
    assert!(!workflow.contains("target/release-artifacts/Ashide-macos.zip"));
}
