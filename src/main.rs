use glib::ControlFlow;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use gtk4::pango;
use gtk4::prelude::*;
use gtk4::{Application, ApplicationWindow, Label};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

// For input events
use evdev::{Device, InputEventKind, Key};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Ensure we have root privileges
    if unsafe { libc::geteuid() } != 0 {
        eprintln!("This program requires root privileges to access input devices.");
        eprintln!("Please run with sudo.");
        return Ok(());
    }

    // Create a channel for key events
    let (tx, rx) = mpsc::channel();

    // Start monitoring keyboard devices in background
    thread::spawn(move || {
        if let Err(e) = monitor_keyboards(tx) {
            eprintln!("Error monitoring keyboards: {}", e);
        }
    });

    // Create GTK application
    let app = Application::builder()
        .application_id("com.hantsaniala.keyprrs")
        .build();

    let rx_arc = Arc::new(Mutex::new(rx));

    let rx_arc_clone = rx_arc.clone();

    app.connect_activate(move |app| {
        // Create a label for displaying key presses
        let label = Label::builder()
            .label("Key Display")
            .margin_top(20)
            .margin_bottom(20)
            .margin_start(20)
            .margin_end(20)
            .halign(gtk4::Align::Center)
            .width_request(100)
            .build();

        // Use a larger font
        label.set_markup("<span font='24' weight='bold'>Key Display</span>");

        label.set_wrap(true); // Enables word wrap
        label.set_max_width_chars(50); // Roughly control width
        label.set_justify(gtk4::Justification::Center);
        label.set_ellipsize(pango::EllipsizeMode::None); // Trim with "..." if overflow

        // Create window
        let window = ApplicationWindow::builder()
            .application(app)
            .title("Key Display")
            .default_width(500)
            .default_height(100)
            .child(&label)
            .build();

        window.set_decorated(false);

        // Set up CSS for transparent background
        let css_provider = gtk4::CssProvider::new();
        css_provider.load_from_data(
            r#"
            window {
                background-color: rgba(0, 0, 0, 0);
            }
            label {
                background-color: rgba(0, 0, 0, 0.5);
                color: white;
                border-radius: 8px;
                padding: 4px 12px;
            }
        "#,
        );

        // Apply CSS to the window
        gtk4::style_context_add_provider_for_display(
            &gtk4::gdk::Display::default().expect("Could not get default display"),
            &css_provider,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );

        // Initialize the window for layer shell
        window.init_layer_shell();

        // Configure it as an overlay
        window.set_layer(Layer::Overlay);

        // Set the window to appear at the top of the screen
        window.set_anchor(Edge::Bottom, true);
        window.set_anchor(Edge::Left, true);
        window.set_anchor(Edge::Right, true);

        // Make the window auto-sized based on contents
        window.set_margin(Edge::Bottom, 50);

        // Set keyboard interaction mode
        window.set_keyboard_mode(KeyboardMode::None);

        // Make the window transparent to mouse clicks
        window.set_exclusive_zone(-1);

        // Present the window
        window.present();

        // Set up a timer to check for key events
        let label_clone = label.clone();
        let rx_arc = rx_arc_clone.clone();

        let visible_keys: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let visible_keys_clone = visible_keys.clone();

        glib::timeout_add_local(Duration::from_millis(50), move || {
            if let Ok(rx) = rx_arc.lock() {
                if let Ok(key_name) = rx.try_recv() {
                    let mut keys = visible_keys_clone.lock().unwrap();
                    keys.push(key_name.clone());

                    // Update label with joined keys
                    label_clone.set_markup(&format!(
                        "<span font='24' weight='bold'>{}</span>",
                        keys.join(" ")
                    ));

                    // Schedule removal after 1 second
                    let visible_keys_inner = visible_keys_clone.clone();
                    let label_inner = label_clone.clone();
                    glib::timeout_add_local_once(Duration::from_secs(1), move || {
                        let mut keys = visible_keys_inner.lock().unwrap();
                        keys.retain(|k| k != &key_name);
                        label_inner.set_markup(&format!(
                            "<span font='24' weight='bold'>{}</span>",
                            keys.join(" ")
                        ));
                    });
                }
            }

            ControlFlow::Continue
        });
    });

    // Run the application
    app.run();

    Ok(())
}

fn monitor_keyboards(tx: mpsc::Sender<String>) -> Result<(), Box<dyn std::error::Error>> {
    // Find keyboard devices
    let devices = find_keyboard_devices()?;

    if devices.is_empty() {
        return Err("No keyboard devices found".into());
    }

    println!("Found {} keyboard devices", devices.len());

    // Start monitoring each device in a separate thread
    for device_path in devices {
        let tx_clone = tx.clone();
        thread::spawn(move || {
            if let Err(e) = monitor_device(&device_path, tx_clone) {
                eprintln!("Error monitoring device {}: {}", device_path.display(), e);
            }
        });
    }

    // Keep the main thread alive
    loop {
        thread::sleep(Duration::from_secs(1));
    }
}

fn find_keyboard_devices() -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    let mut keyboard_devices = Vec::new();

    // Look through /dev/input/event* devices
    for entry in fs::read_dir("/dev/input")? {
        let entry = entry?;
        let path = entry.path();

        if let Some(file_name) = path.file_name() {
            if let Some(name) = file_name.to_str() {
                if name.starts_with("event") {
                    // Try to open the device and check if it's a keyboard
                    if let Ok(device) = Device::open(&path) {
                        // Check if this device has key events
                        if device.supported_events().contains(evdev::EventType::KEY) {
                            keyboard_devices.push(path);
                        }
                    }
                }
            }
        }
    }

    Ok(keyboard_devices)
}

fn monitor_device(
    device_path: &Path,
    tx: mpsc::Sender<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut device = Device::open(device_path)?;

    loop {
        for event in device.fetch_events()? {
            if let InputEventKind::Key(key) = event.kind() {
                if event.value() == 1 {
                    // Key press (not release or repeat)
                    // Convert key code to name
                    let key_name = match key {
                        Key::KEY_A => "a".to_string(),
                        Key::KEY_B => "b".to_string(),
                        Key::KEY_C => "c".to_string(),
                        Key::KEY_D => "d".to_string(),
                        Key::KEY_E => "e".to_string(),
                        Key::KEY_F => "f".to_string(),
                        Key::KEY_G => "g".to_string(),
                        Key::KEY_H => "h".to_string(),
                        Key::KEY_I => "i".to_string(),
                        Key::KEY_J => "j".to_string(),
                        Key::KEY_K => "k".to_string(),
                        Key::KEY_L => "l".to_string(),
                        Key::KEY_M => "m".to_string(),
                        Key::KEY_N => "n".to_string(),
                        Key::KEY_O => "o".to_string(),
                        Key::KEY_P => "p".to_string(),
                        Key::KEY_Q => "q".to_string(),
                        Key::KEY_R => "r".to_string(),
                        Key::KEY_S => "s".to_string(),
                        Key::KEY_T => "t".to_string(),
                        Key::KEY_U => "u".to_string(),
                        Key::KEY_V => "v".to_string(),
                        Key::KEY_W => "w".to_string(),
                        Key::KEY_X => "x".to_string(),
                        Key::KEY_Y => "y".to_string(),
                        Key::KEY_Z => "z".to_string(),
                        Key::KEY_0 => "0".to_string(),
                        Key::KEY_1 => "1".to_string(),
                        Key::KEY_2 => "2".to_string(),
                        Key::KEY_3 => "3".to_string(),
                        Key::KEY_4 => "4".to_string(),
                        Key::KEY_5 => "5".to_string(),
                        Key::KEY_6 => "6".to_string(),
                        Key::KEY_7 => "7".to_string(),
                        Key::KEY_8 => "8".to_string(),
                        Key::KEY_9 => "9".to_string(),
                        Key::KEY_SPACE => "˽".to_string(),
                        Key::KEY_ENTER => "⤶".to_string(),
                        Key::KEY_TAB => "↹".to_string(),
                        Key::KEY_ESC => "Esc".to_string(),
                        Key::KEY_BACKSPACE => "Backspace".to_string(),
                        Key::KEY_LEFTSHIFT => "L🠹".to_string(),
                        Key::KEY_RIGHTSHIFT => "R🠹".to_string(),
                        Key::KEY_LEFTCTRL => "Left Ctrl".to_string(),
                        Key::KEY_RIGHTCTRL => "Right Ctrl".to_string(),
                        Key::KEY_LEFTALT => "Left Alt".to_string(),
                        Key::KEY_RIGHTALT => "Right Alt".to_string(),
                        _ => format!("{:?}", key),
                    };

                    // Send key name to main thread
                    if let Err(e) = tx.send(key_name) {
                        eprintln!("Error sending key: {}", e);
                    }
                }
            }
        }

        // Sleep a bit to avoid high CPU usage
        thread::sleep(Duration::from_millis(10));
    }
}
