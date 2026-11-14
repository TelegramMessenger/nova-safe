#!/bin/sh
# Links the prelinked Nova library the way Telegram's binary does: next to
# Objective-C and C++ code that throws and another Rust static library with its
# own rust_eh_personality. The link must succeed in both library orders, the
# image must keep at most three personality routines, and a panic in the other
# Rust library must still be caught.
#
#   safe/embed/check-personality.sh LIBNOVA RUSTC WORK_DIR

set -eu

LIB="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
RUSTC_BIN="$2"
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
rm -rf "$3"
mkdir -p "$3"
WORK="$(cd "$3" && pwd)"
cd "$WORK"

cat > other.rs <<'RS'
use std::panic::catch_unwind;
struct Guard;
impl Drop for Guard { fn drop(&mut self) {} }
#[inline(never)]
fn deep(n: u32) -> u32 { let _g = Guard; if n == 0 { panic!("boom") } deep(n - 1) + 1 }
#[unsafe(no_mangle)]
pub extern "C" fn other_rust_probe(n: u32) -> i32 {
    std::panic::set_hook(Box::new(|_| {}));
    match catch_unwind(|| deep(n)) { Ok(_) => 0, Err(e) => if e.downcast_ref::<&str>() == Some(&"boom") { 1 } else { 2 } }
}
RS
cat > cxx.cpp <<'CPP'
#include <stdexcept>
extern "C" int cxx_probe(int x) { try { if (x) throw std::runtime_error("x"); return 0; } catch (...) { return 1; } }
CPP
cat > objc.m <<'OBJC'
#import <Foundation/Foundation.h>
int objc_probe(int x) { @try { if (x) @throw [NSException exceptionWithName:@"n" reason:@"r" userInfo:nil]; return 0; } @catch (NSException *e) { return 1; } }
OBJC
cat > main.c <<'C'
#include <stdio.h>
#include <stdlib.h>
int other_rust_probe(unsigned); int cxx_probe(int); int objc_probe(int);
const char *nova_embed_version(void);
int main(void) {
    int ok = other_rust_probe(3) == 1 && cxx_probe(1) == 1 && objc_probe(1) == 1 && nova_embed_version() != NULL;
    puts(ok ? "probes ok" : "probes failed");
    return ok ? 0 : 1;
}
C

for pair in "arm64 aarch64-apple-darwin 11.0" "x86_64 x86_64-apple-darwin 10.13"; do
    set -- $pair
    arch="$1"; target="$2"; deployment="$3"
    (cd "$ROOT" && "$RUSTC_BIN" --crate-type staticlib -C opt-level=2 --target "$target" "$WORK/other.rs" -o "$WORK/libother-$arch.a")
    clang -c -arch "$arch" -mmacosx-version-min="$deployment" cxx.cpp -o "cxx-$arch.o"
    clang -c -arch "$arch" -mmacosx-version-min="$deployment" -fobjc-exceptions objc.m -o "objc-$arch.o"
    clang -c -arch "$arch" -mmacosx-version-min="$deployment" main.c -o "main-$arch.o"
    for order in nova-first other-first; do
        if [ "$order" = nova-first ]; then
            first="$LIB"; second="libother-$arch.a"
        else
            first="libother-$arch.a"; second="$LIB"
        fi
        if ! clang++ -arch "$arch" -mmacosx-version-min="$deployment" "main-$arch.o" "cxx-$arch.o" "objc-$arch.o" \
            "$first" "$second" -framework Foundation -o "probe-$arch-$order" 2> "link-$arch-$order.log"; then
            cat "link-$arch-$order.log"
            echo "$arch ($order): the link failed"
            exit 1
        fi
        count="$(objdump --unwind-info "probe-$arch-$order" | sed -n 's/.*Personality functions: (count = \([0-9]*\)).*/\1/p')"
        if [ -z "$count" ] || [ "$count" -gt 3 ]; then
            echo "$arch ($order): ${count:-unknown} personality routines"
            exit 1
        fi
        if [ "$arch" = "$(uname -m)" ] || arch -x86_64 /usr/bin/true 2>/dev/null; then
            "./probe-$arch-$order" >/dev/null || { echo "$arch ($order): the probes failed"; exit 1; }
        fi
        echo "$arch ($order): links with $count personality routines"
    done
done
