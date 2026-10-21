#!/bin/bash
zip=$(ls -t /c/Users/jmilburn/Downloads/playground-*.zip | head -1)
echo "Installing: $zip"
unzip -o "$zip" -d /c/Playground && cargo build --release && ./target/release/playground.exe
