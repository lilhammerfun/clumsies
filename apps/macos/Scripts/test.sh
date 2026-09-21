#!/bin/sh

set -eu

result_root=$(mktemp -d "${TMPDIR:-/private/tmp}/clumsies-macos-test-results.XXXXXX")
trap 'rm -rf -- "$result_root"' EXIT

xcodegen generate --spec apps/macos/project.yml

test_language() {
    language="$1"
    shift
    result_bundle="$result_root/Clumsies-$language.xcresult"
    set +e
    CLUMSIES_SKIP_DAEMON_BUILD=1 xcodebuild -quiet \
        -project apps/macos/Clumsies.xcodeproj \
        -scheme Clumsies \
        -configuration Debug \
        -derivedDataPath /private/tmp/clumsies-macos-tests \
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
