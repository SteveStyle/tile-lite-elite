# 🎯 Scrabble Web & Desktop App

A modern Scrabble game implementation built with **Dioxus** (Rust) that runs both as a web application and native desktop app.

## ✨ Features

- 🎲 **Full 15x15 Scrabble Board** with authentic premium squares
- 🎨 **Multiple Board Layouts**:
  - Traditional Scrabble (US/UK/International standard)
  - Wordfeud variant (more double letter squares)
- 🖱️ **Interactive Gameplay** - Click tiles to place letters
- 📱 **Responsive Design** - Works on desktop and mobile browsers
- 🖥️ **Native Desktop App** - True Windows .exe application
- 🎯 **Premium Square Highlighting** - Visual indicators for special squares

## 🚀 Quick Start

### Web Version
```bash
# Install Dioxus CLI
cargo install dioxus-cli

# Run development server
dx serve --port 3000

# Open browser to http://127.0.0.1:3000
```

### Desktop Version (Windows)
```bash
# Build for desktop
cargo build --release --features desktop

# Run desktop app
./target/release/scrabble-px
```

## 🛠️ Development

### Project Structure
```
scrabble-px/
├─ assets/                     # Static assets and styling
│  ├─ favicon.ico
│  ├─ header.svg
│  └─ styling/                # CSS files
│     ├─ main.css            # Main application styles
│     ├─ navbar.css
│     └─ echo.css
├─ src/
│  ├─ main.rs                # Application entry point
│  ├─ components/            # Reusable UI components
│  │  ├─ mod.rs
│  │  ├─ scrabble_board.rs   # Main game board component
│  │  ├─ hero.rs
│  │  └─ echo.rs
│  └─ views/                 # Page views
│     ├─ mod.rs
│     ├─ home.rs
│     ├─ blog.rs
│     └─ navbar.rs
├─ Cargo.toml               # Dependencies and configuration
├─ Dioxus.toml             # Dioxus framework configuration
└─ build-windows.sh        # Windows build helper script
```

### Technologies Used

- **[Dioxus](https://dioxuslabs.com/)** - Rust-based UI framework
- **WebView2** - Native Windows desktop rendering
- **CSS Grid/Flexbox** - Responsive board layout
- **Rust/WebAssembly** - High-performance game logic

### Board Layout Implementation

The game implements authentic Scrabble board layouts with precise premium square positioning:

- **Triple Word Score** (red): Corners and center cross
- **Double Word Score** (pink): Diagonal pattern from center
- **Triple Letter Score** (blue): Strategic positions for high-value letters  
- **Double Letter Score** (light blue): Common letter multiplier positions

## 🎮 How to Play

1. **Select Board Layout**: Choose between Traditional or Wordfeud
2. **Place Tiles**: Click on board squares to place letter tiles
3. **Premium Squares**: Take advantage of multiplier squares for higher scores
4. **Interactive**: Full click-to-place tile system

## 🔧 Configuration

### Web Features
```toml
[features]
default = ["web"]
web = ["dioxus/web"]
```

### Desktop Features  
```toml
[features]
default = ["desktop"]
desktop = ["dioxus/desktop"]
```

## 📦 Building for Distribution

### Web Build
```bash
dx build --release
# Files output to dist/ directory
```

### Windows Desktop Build
```bash
# Use the provided script for Windows-ready build
./build-windows.sh

# Or manually:
cargo build --release --features desktop
```

## 🚀 Deployment

### Web Deployment
The web version can be deployed to any static hosting service:
- GitHub Pages
- Netlify  
- Vercel
- Apache/Nginx

### Desktop Distribution
The Windows desktop version creates a standalone `.exe` file that can be:
- Distributed directly to users
- Added to Windows Start Menu
- Packaged with an installer

## 🤝 Contributing

1. Fork the repository
2. Create a feature branch (`git checkout -b feature/amazing-feature`)
3. Commit your changes (`git commit -m 'Add amazing feature'`)
4. Push to the branch (`git push origin feature/amazing-feature`)
5. Open a Pull Request

## 📄 License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.

## 🎯 Roadmap

- [ ] Score calculation system
- [ ] Dictionary validation
- [ ] Multiplayer support
- [ ] Game save/load functionality
- [ ] AI opponent
- [ ] Custom tile sets
- [ ] Sound effects
- [ ] Game history tracking

## 🐛 Known Issues

- None currently reported

## 📞 Support

If you encounter any issues or have questions:
1. Check the [Issues](../../issues) page
2. Create a new issue with detailed information
3. Include your OS, browser, and Rust version

---

**Built with ❤️ using Rust and Dioxus**
│  ├─ components/
│  │  ├─ mod.rs # Defines the components module
│  │  ├─ hero.rs # The Hero component for use in the home page
│  │  ├─ echo.rs # The echo component uses server functions to communicate with the server
│  ├─ views/ # The views each route will render in the app.
│  │  ├─ mod.rs # Defines the module for the views route and re-exports the components for each route
│  │  ├─ blog.rs # The component that will render at the /blog/:id route
│  │  ├─ home.rs # The component that will render at the / route
├─ Cargo.toml # The Cargo.toml file defines the dependencies and feature flags for your project
```

### Serving Your App

Run the following command in the root of your project to start developing with the default platform:

```bash
dx serve --platform web
```

To run for a different platform, use the `--platform platform` flag. E.g.
```bash
dx serve --platform desktop
```

