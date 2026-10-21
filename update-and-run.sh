#!/bin/bash
zip=$(ls -t /c/Users/jmilburn/Downloads/playground-*.zip | head -1)
echo "Extracting: $zip"
unzip -o "$zip" -d /c/Playground || exit 1
cargo build --release || exit 1
./target/release/fs-mod-manager.exe
