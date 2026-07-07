#!/bin/bash
# Linux Desktop-Style Launcher for Scrabble App

echo "🎯 Starting Scrabble Desktop-Style App on Linux..."

# Function to check if a command exists
command_exists() {
    command -v "$1" >/dev/null 2>&1
}

# Function to launch in desktop mode
launch_desktop_mode() {
    local browser="$1"
    local url="http://127.0.0.1:3000"
    
    echo "🚀 Launching Scrabble in desktop mode using $browser..."
    
    case "$browser" in
        "google-chrome"|"chromium")
            "$browser" --app="$url" --window-size=1000,800 --disable-web-security --user-data-dir=/tmp/scrabble-app &
            ;;
        "firefox")
            "$browser" --kiosk "$url" &
            ;;
        *)
            echo "⚠️  Opening in regular browser window..."
            "$browser" "$url" &
            ;;
    esac
}

# Start the development server in background
echo "📡 Starting Scrabble development server..."
cd "$(dirname "$0")"

# Check if dx is available
if command_exists dx; then
    dx serve --port 3000 &
    SERVER_PID=$!
    echo "✅ Server started with PID: $SERVER_PID"
elif command_exists cargo; then
    echo "⚠️  Dioxus CLI (dx) not found. Falling back to native desktop launch via Cargo..."
    cargo run --features desktop &
    SERVER_PID=$!
    echo "✅ Desktop app started with PID: $SERVER_PID"
else
    echo "❌ Neither Dioxus CLI (dx) nor Cargo was found."
    echo "   Install dx with: cargo install dioxus-cli"
    exit 1
fi

if command_exists dx; then
    # Wait for server to start before opening the browser.
    echo "⏳ Waiting for server to start..."
    sleep 3

    # Detect available browsers and launch in desktop mode
    if command_exists google-chrome; then
        launch_desktop_mode "google-chrome"
    elif command_exists chromium; then
        launch_desktop_mode "chromium"
    elif command_exists firefox; then
        launch_desktop_mode "firefox"
    else
        echo "❌ No suitable browser found. Please install Chrome, Chromium, or Firefox."
        echo "   Opening server at: http://127.0.0.1:3000"
        echo "   You can manually open this URL in your browser."
    fi
fi

echo ""
echo "🎮 Scrabble Desktop App is now running!"
echo "📱 Features available:"
echo "   • Full 15x15 Scrabble board"
echo "   • Traditional and Wordfeud layouts"
echo "   • Premium square highlighting"
echo "   • Interactive tile placement"
echo ""
echo "🛑 To stop the app, press Ctrl+C or close the browser window"
echo "   Server PID: $SERVER_PID"

# Keep script running until interrupted
trap "echo ''; echo '🛑 Stopping Scrabble server...'; kill $SERVER_PID 2>/dev/null; exit 0" INT

# Wait for server process
wait $SERVER_PID