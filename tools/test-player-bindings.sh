#!/usr/bin/env bash
# Builds the player crate's UniFFI bindings and runs the scenario smoke test on
# Swift and Kotlin. See crates/player/README.md.
#
# Everything the run produces, including the pinned tools it downloads, lives under
# <cargo target dir>/player-bindings; the source tree is left untouched.
set -euo pipefail

usage() {
	echo "usage: $0 [--swift] [--kotlin]" >&2
	echo "Runs the selected targets (default: all)." >&2
	exit 2
}

targets=()
for arg in "$@"; do
	case "$arg" in
	--swift) targets+=(swift) ;;
	--kotlin) targets+=(kotlin) ;;
	*) usage ;;
	esac
done
[[ ${#targets[@]} -gt 0 ]] || targets=(swift kotlin)

cd "$(dirname "$0")/.."
root=$PWD
command -v node >/dev/null || { echo "node not found; install Node (brew install node)" >&2; exit 1; }
# Cargo's own answer, absolute and honouring CARGO_TARGET_DIR and build.target-dir. Export it
# so every cargo call, including those made by ubrn, uses this directory.
target_dir=$(cargo metadata --format-version 1 --no-deps |
	node -e 'process.stdout.write(JSON.parse(require("fs").readFileSync(0, "utf8")).target_directory)')
export CARGO_TARGET_DIR=$target_dir
out=$target_dir/player-bindings
tools=$out/tools
tests=$root/crates/player/tests/bindings

maven=https://repo1.maven.org/maven2
kotlin_version=2.4.0
# path (relative to the repository) and SHA-256 of each pinned jar.
kotlin_compiler_jars=(
	"org/jetbrains/kotlin/kotlin-compiler/$kotlin_version/kotlin-compiler-$kotlin_version.jar 66e73eacd619c9beb7c22042117af36b443529c4d80237ee82cc4b2acb6f3d0b"
	"org/jetbrains/kotlin/kotlin-script-runtime/$kotlin_version/kotlin-script-runtime-$kotlin_version.jar 196e8ecbf63daf477e34e67c2f871ee232ae7bf8b377432d876d1bbf9d273de3"
	"org/jetbrains/kotlin/kotlin-reflect/1.6.10/kotlin-reflect-1.6.10.jar 3277ac102ae17aad10a55abec75ff5696c8d109790396434b496e75087854203"
	"org/jetbrains/kotlin/kotlin-build-tools-api/$kotlin_version/kotlin-build-tools-api-$kotlin_version.jar 66917335497f9ab75a36c34b3d90bad2bc8dc256c34b2413ecd0c9467fcf597d"
	"org/jetbrains/annotations/13.0/annotations-13.0.jar ace2a10dc8e2d5fd34925ecac03e4988b2c0f851650c94b8cef49ba1bd111478"
)
# Needed both by the compiler and by the compiled test.
kotlin_runtime_jars=(
	"org/jetbrains/kotlin/kotlin-stdlib/$kotlin_version/kotlin-stdlib-$kotlin_version.jar ccb14ff83fabcb11458b798dbc9824748ccdffeec79c9aba789e6ed1cda86b1c"
	"org/jetbrains/kotlinx/kotlinx-coroutines-core-jvm/1.8.0/kotlinx-coroutines-core-jvm-1.8.0.jar 9860906a1937490bf5f3b06d2f0e10ef451e65b95b269f22daf68a3d1f5065c5"
	"net/java/dev/jna/jna/5.13.0/jna-5.13.0.jar 66d4f819a062a51a1d5627bffc23fac55d1677f0e0a1feba144aabdd670a64bb"
)

case "$(uname -s)" in
Darwin) library=libplayer.dylib ;;
*) library=libplayer.so ;;
esac

fail() {
	echo "$1" >&2
	exit 1
}

require() {
	command -v "$1" >/dev/null || fail "$1 not found; $2"
}

# Downloads each "<path> <sha256>" jar into $tools/kotlin and sets `jars` to their locations.
fetch_jars() {
	local entry path sha jar
	jars=()
	mkdir -p "$tools/kotlin"
	for entry in "$@"; do
		read -r path sha <<<"$entry"
		jar=$tools/kotlin/$(basename "$path")
		if [[ ! -f $jar ]]; then
			curl -fsSL "$maven/$path" -o "$jar.part"
			[[ $(shasum -a 256 "$jar.part" | cut -d' ' -f1) == "$sha" ]] ||
				fail "checksum mismatch for $path"
			mv "$jar.part" "$jar"
		fi
		jars+=("$jar")
	done
}

# Prints the arguments joined with ':' as a classpath.
join_classpath() {
	local IFS=:
	echo "$*"
}

build_player() {
	cargo build --locked -p player --features uniffi-bindgen
}

generate() {
	"$target_dir/debug/player-bindgen" generate --library "$target_dir/debug/$library" \
		--language "$1" --out-dir "$out/$1/bindings" --no-format
}

run_swift() {
	require swiftc "install Xcode or a Swift toolchain"
	generate swift
	local bindings=$out/swift/bindings
	cp "$bindings/playerFFI.modulemap" "$bindings/module.modulemap"
	swiftc -swift-version 5 -parse-as-library -I "$bindings" \
		-L "$target_dir/debug" -lplayer -Xlinker -rpath -Xlinker "$target_dir/debug" \
		"$bindings/player.swift" "$tests/Smoke.swift" -o "$out/swift/smoke"
	"$out/swift/smoke"
}

run_kotlin() {
	require java "install a JDK"
	generate kotlin
	local runtime_jars compiler runtime
	fetch_jars "${kotlin_runtime_jars[@]}"
	runtime_jars=("${jars[@]}")
	runtime=$(join_classpath "${runtime_jars[@]}")
	fetch_jars "${kotlin_compiler_jars[@]}"
	compiler=$(join_classpath "${jars[@]}" "${runtime_jars[@]}")
	rm -rf "$out/kotlin/classes"
	java -cp "$compiler" org.jetbrains.kotlin.cli.jvm.K2JVMCompiler -no-stdlib -no-reflect \
		-classpath "$runtime" "$out/kotlin/bindings/uniffi/player/player.kt" "$tests/Smoke.kt" \
		-d "$out/kotlin/classes"
	java -Djna.library.path="$target_dir/debug" -cp "$out/kotlin/classes:$runtime" SmokeKt
}

results=()
status=0
build_player || fail "FAIL build: could not build the player crate"
set +e
for target in "${targets[@]}"; do
	echo "== $target"
	(
		set -e
		"run_$target"
	)
	if [[ $? -eq 0 ]]; then
		results+=("PASS $target")
	else
		results+=("FAIL $target")
		status=1
	fi
done
set -e

echo
printf '%s\n' "${results[@]}"
exit "$status"
