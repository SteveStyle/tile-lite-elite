#!/bin/bash
# Windows Desktop Build Script for Scrabble App

echo "🎯 Building Scrabble Desktop App for Windows..."

# Check if we're in the right directory
if [ ! -f "Cargo.toml" ]; then
    echo "❌ Error: Please run this script from the scrabble-px directory"
    exit 1
fi

# Copy project to Windows-compatible structure
echo "📁 Preparing Windows build directory..."
mkdir -p ../scrabble-windows-build
cp -r . ../scrabble-windows-build/

cd ../scrabble-windows-build

# Update Cargo.toml for desktop-only build
cat > Cargo.toml << 'EOF'
[package]
name = "scrabble-px"
version = "0.1.0"
authors = ["vagrant"]
edition = "2021"

[dependencies]
dioxus = { version = "0.6.0", features = ["desktop"] }
dioxus-desktop = "0.6.0"

[features]
default = ["desktop"]
desktop = ["dioxus/desktop"]

[profile.release]
opt-level = 3
lto = true
codegen-units = 1
panic = "abort"
EOF

echo "✅ Windows build directory created at: ../scrabble-windows-build"
echo ""
echo "🎯 Next steps:"
echo "1. Copy the 'scrabble-windows-build' folder to your Windows host"
echo "2. On Windows, install Rust: https://rustup.rs/"
echo "3. Run: cargo build --release"
echo "4. Your .exe will be in: target/release/scrabble-px.exe"
echo ""
echo "🖥️  Windows Desktop Features:"
echo "   • Native .exe file"
echo "   • Resizable window (800x600 to 1000x800)"
echo "   • Windows taskbar integration"
echo "   • No browser required"