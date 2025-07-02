# 🎚️ Radio Conductor

**Radio Conductor** is a simple and fast tool for managing and launching music playback — perfect for radio stations, DJs, or anyone needing precise and responsive music triggering.

## ✨ Features

* 🎵 **Instant Playback**: Play MP3 and WAV files at the click of a button.
* ⏱️ **Track Timer**: See how much time is left for the currently playing track.
* 🎚️ **Smooth Transitions**: Click a playing button again to fade out and stop playback (1-second fade). Clicking another button will start the new track immediately.
* 🎨 **Customizable Buttons**: Change button colors and labels for quick identification.
* 🗂️ **Tab Organization**: Group buttons under customizable tabs.
* 💾 **Save & Load Configurations**: Export and import button setups (note: file paths must remain the same).

## 🚧 TODO

* Improve audio buffering to speed up import time
* Add support for trimming/selecting portions of a track
* Allow selection of audio output device (currently uses system default)

## 🛠️ Built With

* 🦀 **Rust** — Fast, reliable, and safe systems programming.

## 📦 Installation

```bash
# Clone the repo
git clone https://github.com/your-username/radio-conductor.git
cd radio-conductor

# Build the project
cargo build --release
```

## 🚀 Usage

Launch the application and start importing your music. Assign them to buttons, customize labels/colors, and organize your tabs.

## 📁 Configuration

You can export your current setup and later import it. Make sure music files remain in the same location to restore properly.
