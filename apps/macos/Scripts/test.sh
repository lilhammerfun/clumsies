#!/bin/sh

set -eu

derived_data="${CLUMSIES_MACOS_DERIVED_DATA:-/private/tmp/clumsies-macos-tests}"
result_root=$(mktemp -d "${TMPDIR:-/private/tmp}/clumsies-macos-test-results.XXXXXX")
trap 'rm -rf -- "$result_root"' EXIT
test_preferences_home="$result_root/home"
mkdir -p "$test_preferences_home/Library/Preferences"

xcodegen generate --spec apps/macos/project.yml

test_language() {
    language="$1"
    shift
    result_bundle="$result_root/Clumsies-$language.xcresult"
    set +e
    CFFIXED_USER_HOME="$test_preferences_home" \
        TEST_RUNNER_CFFIXED_USER_HOME="$test_preferences_home" \
        TEST_RUNNER_CLUMSIES_ISOLATED_TEST_HOME="$test_preferences_home" \
        CLUMSIES_SKIP_DAEMON_BUILD=1 xcodebuild -quiet \
        -project apps/macos/Clumsies.xcodeproj \
        -scheme Clumsies \
        -configuration Debug \
        -derivedDataPath "$derived_data" \
        -resultBundlePath "$result_bundle" \
        -testLanguage "$language" \
        "$@" test
    status=$?
    set -e
    if [ "$status" -ne 0 ]; then
        xcrun xcresulttool get test-results summary --path "$result_bundle" || true
        exit "$status"
    fi
}

test_language en
test_language zh-Hans -only-testing:ClumsiesTests/LocalizationTests \
    -only-testing:ClumsiesTests/AppLanguageTests \
    -only-testing:ClumsiesTests/SettingsWindowLayoutTests
