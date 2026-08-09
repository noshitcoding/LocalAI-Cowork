use std::{path::Path, process::Stdio, time::Duration};

use anyhow::{bail, Context, Result};
#[cfg(windows)]
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde_json::{json, Value};
use tokio::{process::Command, time::timeout};

pub fn available() -> bool {
    #[cfg(windows)]
    {
        true
    }
    #[cfg(target_os = "linux")]
    {
        std::env::var_os("DISPLAY").is_some()
            && command_available("xdotool")
            && (command_available("gnome-screenshot") || command_available("import"))
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        false
    }
}

pub async fn execute(action: &str, arguments: &Value, screenshot_path: &Path) -> Result<Value> {
    #[cfg(windows)]
    {
        execute_windows(action, arguments, screenshot_path).await
    }
    #[cfg(target_os = "linux")]
    {
        execute_linux(action, arguments, screenshot_path).await
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        bail!("desktop automation is unsupported on this operating system")
    }
}

#[cfg(windows)]
async fn execute_windows(action: &str, arguments: &Value, screenshot_path: &Path) -> Result<Value> {
    let payload = json!({"arguments":arguments,"screenshot_path":screenshot_path});
    let body = match action {
        "display" => format!(
            "{}\n$b=[System.Windows.Forms.Screen]::PrimaryScreen.Bounds;@{{x=$b.X;y=$b.Y;width=$b.Width;height=$b.Height;primary=$true;device_name=[System.Windows.Forms.Screen]::PrimaryScreen.DeviceName;scale_factor=1}}|ConvertTo-Json -Compress",
            windows_forms_prelude()
        ),
        "screenshot" => format!(
            "{}\n$b=[System.Windows.Forms.Screen]::PrimaryScreen.Bounds;$bitmap=New-Object System.Drawing.Bitmap($b.Width,$b.Height);$graphics=[System.Drawing.Graphics]::FromImage($bitmap);try{{$graphics.CopyFromScreen($b.Location,[System.Drawing.Point]::Empty,$b.Size);$parent=[IO.Path]::GetDirectoryName($p.screenshot_path);[IO.Directory]::CreateDirectory($parent)|Out-Null;$bitmap.Save($p.screenshot_path,[System.Drawing.Imaging.ImageFormat]::Png)}}finally{{$graphics.Dispose();$bitmap.Dispose()}};@{{path=$p.screenshot_path;x=$b.X;y=$b.Y;width=$b.Width;height=$b.Height;primary=$true;device_name=[System.Windows.Forms.Screen]::PrimaryScreen.DeviceName;scale_factor=1}}|ConvertTo-Json -Compress",
            windows_forms_prelude()
        ),
        "list_windows" => format!(
            "{}\n$items=@();Get-Process|Where-Object{{$_.MainWindowHandle-ne0-and$_.MainWindowTitle}}|ForEach-Object{{$items+=@{{title=$_.MainWindowTitle;process_name=$_.ProcessName;process_id=$_.Id;handle=[int64]$_.MainWindowHandle}}}};$items|ConvertTo-Json -Compress",
            windows_forms_prelude()
        ),
        "focus_window" => format!(
            "{}\n$target=Get-Process|Where-Object{{$_.MainWindowHandle-ne0-and((($p.arguments.process_id-ne$null)-and$_.Id-eq[int]$p.arguments.process_id)-or(($p.arguments.process_name)-and$_.ProcessName-like('*'+$p.arguments.process_name+'*'))-or(($p.arguments.title)-and$_.MainWindowTitle-like('*'+$p.arguments.title+'*')))}}|Select-Object -First 1;if($null-eq$target){{throw 'matching desktop window was not found'}};[CoworkDesktop]::ShowWindow($target.MainWindowHandle,9)|Out-Null;[CoworkDesktop]::SetForegroundWindow($target.MainWindowHandle)|Out-Null;@{{title=$target.MainWindowTitle;process_name=$target.ProcessName;process_id=$target.Id;focused=$true}}|ConvertTo-Json -Compress",
            windows_native_prelude()
        ),
        "launch" => {
            "$argsList=@($p.arguments.args);$options=@{FilePath=[string]$p.arguments.app_path;PassThru=$true};if($argsList.Count-gt0){$options.ArgumentList=$argsList};if($p.arguments.cwd){$options.WorkingDirectory=[string]$p.arguments.cwd};$process=Start-Process @options;@{path=$p.arguments.app_path;process_id=$process.Id;launched=$true}|ConvertTo-Json -Compress".to_owned()
        }
        "move_mouse" => format!(
            "{}\n$x=[int]$p.arguments.x;$y=[int]$p.arguments.y;if($p.arguments.coordinate_space-eq'display'){{$b=[System.Windows.Forms.Screen]::PrimaryScreen.Bounds;$x+=$b.X;$y+=$b.Y}};if(-not[CoworkDesktop]::SetCursorPos($x,$y)){{throw 'SetCursorPos failed'}};@{{x=$x;y=$y;moved=$true}}|ConvertTo-Json -Compress",
            windows_desktop_prelude()
        ),
        "click" => format!(
            "{}\n$x=[int]$p.arguments.x;$y=[int]$p.arguments.y;if($p.arguments.coordinate_space-eq'display'){{$b=[System.Windows.Forms.Screen]::PrimaryScreen.Bounds;$x+=$b.X;$y+=$b.Y}};[CoworkDesktop]::SetCursorPos($x,$y)|Out-Null;$right=$p.arguments.button-eq'right';$down=if($right){{0x0008}}else{{0x0002}};$up=if($right){{0x0010}}else{{0x0004}};$count=if($p.arguments.double_click){{2}}else{{1}};1..$count|ForEach-Object{{[CoworkDesktop]::mouse_event($down,0,0,0,[UIntPtr]::Zero);[CoworkDesktop]::mouse_event($up,0,0,0,[UIntPtr]::Zero);Start-Sleep -Milliseconds 80}};@{{x=$x;y=$y;button=if($right){{'right'}}else{{'left'}};clicked=$true;click_count=$count}}|ConvertTo-Json -Compress",
            windows_desktop_prelude()
        ),
        "type_text" => format!(
            "{}\n[System.Windows.Forms.SendKeys]::SendWait([string]$p.arguments.send_keys);@{{typed=$true;characters=([string]$p.arguments.text).Length}}|ConvertTo-Json -Compress",
            windows_forms_prelude()
        ),
        "keypress" => format!(
            "{}\n[System.Windows.Forms.SendKeys]::SendWait([string]$p.arguments.send_keys);@{{pressed=$true;keys=$p.arguments.keys}}|ConvertTo-Json -Compress",
            windows_forms_prelude()
        ),
        "scroll" => format!(
            "{}\nif($p.arguments.x-ne$null-and$p.arguments.y-ne$null){{[CoworkDesktop]::SetCursorPos([int]$p.arguments.x,[int]$p.arguments.y)|Out-Null}};$delta=[int](-120*[Math]::Sign([double]$p.arguments.scroll_y)*[Math]::Max(1,[Math]::Ceiling([Math]::Abs([double]$p.arguments.scroll_y)/120)));[CoworkDesktop]::mouse_event(0x0800,0,0,$delta,[UIntPtr]::Zero);@{{scrolled=$true;delta=$delta}}|ConvertTo-Json -Compress",
            windows_native_prelude()
        ),
        other => bail!("unsupported desktop action {other}"),
    };
    run_powershell(&payload, &body).await
}

#[cfg(windows)]
fn windows_forms_prelude() -> &'static str {
    "Add-Type -AssemblyName System.Windows.Forms;Add-Type -AssemblyName System.Drawing"
}

#[cfg(windows)]
fn windows_native_prelude() -> &'static str {
    r#"Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class CoworkDesktop {
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hWnd, int command);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint flags, uint dx, uint dy, int data, UIntPtr extra);
}
'@"#
}

#[cfg(windows)]
fn windows_desktop_prelude() -> String {
    format!("{}\n{}", windows_forms_prelude(), windows_native_prelude())
}

#[cfg(windows)]
async fn run_powershell(payload: &Value, body: &str) -> Result<Value> {
    let encoded = BASE64.encode(serde_json::to_vec(payload)?);
    let script = format!(
        "$ErrorActionPreference='Stop';$p=[Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('{encoded}'))|ConvertFrom-Json;{body}"
    );
    let output = run_output(
        "powershell.exe",
        &["-NoProfile", "-NonInteractive", "-Command", &script],
    )
    .await?;
    serde_json::from_str(output.trim()).context("desktop command returned invalid JSON")
}

#[cfg(target_os = "linux")]
async fn execute_linux(action: &str, arguments: &Value, screenshot_path: &Path) -> Result<Value> {
    match action {
        "display" => linux_display().await,
        "screenshot" => {
            if let Some(parent) = screenshot_path.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            if command_available("gnome-screenshot") {
                run_output(
                    "gnome-screenshot",
                    &["-f", screenshot_path.to_string_lossy().as_ref()],
                )
                .await?;
            } else {
                run_output(
                    "import",
                    &[
                        "-window",
                        "root",
                        screenshot_path.to_string_lossy().as_ref(),
                    ],
                )
                .await?;
            }
            let display = linux_display().await?;
            Ok(
                json!({"path":screenshot_path,"x":0,"y":0,"width":display["width"],"height":display["height"],"primary":true,"device_name":"DISPLAY","scale_factor":1}),
            )
        }
        "list_windows" => {
            let ids = run_output("xdotool", &["search", "--onlyvisible", "--name", ".*"]).await?;
            let mut windows = Vec::new();
            for id in ids.lines().take(500) {
                let title = run_output("xdotool", &["getwindowname", id])
                    .await
                    .unwrap_or_default();
                let pid = run_output("xdotool", &["getwindowpid", id])
                    .await
                    .unwrap_or_default();
                windows.push(json!({"handle":id,"title":title.trim(),"process_id":pid.trim().parse::<u32>().ok()}));
            }
            Ok(Value::Array(windows))
        }
        "focus_window" => {
            let title = string_argument(arguments, "title")?;
            let id = run_output("xdotool", &["search", "--onlyvisible", "--name", title])
                .await?
                .lines()
                .next()
                .context("matching desktop window was not found")?
                .to_owned();
            run_output("xdotool", &["windowactivate", "--sync", &id]).await?;
            Ok(json!({"handle":id,"title":title,"focused":true}))
        }
        "launch" => {
            let program = string_argument(arguments, "app_path")?;
            let args = arguments
                .get("args")
                .and_then(Value::as_array)
                .map(|items| items.iter().filter_map(Value::as_str).collect::<Vec<_>>())
                .unwrap_or_default();
            let mut command = Command::new(program);
            command
                .args(&args)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            if let Some(cwd) = arguments.get("cwd").and_then(Value::as_str) {
                command.current_dir(cwd);
            }
            let child = command.spawn()?;
            Ok(json!({"path":program,"process_id":child.id(),"launched":true}))
        }
        "move_mouse" => {
            let (x, y) = coordinates(arguments)?;
            run_output("xdotool", &["mousemove", &x.to_string(), &y.to_string()]).await?;
            Ok(json!({"x":x,"y":y,"moved":true}))
        }
        "click" => {
            let (x, y) = coordinates(arguments)?;
            let button = if arguments.get("button").and_then(Value::as_str) == Some("right") {
                "3"
            } else {
                "1"
            };
            let repeat = if arguments.get("double_click").and_then(Value::as_bool) == Some(true) {
                "2"
            } else {
                "1"
            };
            run_output(
                "xdotool",
                &[
                    "mousemove",
                    &x.to_string(),
                    &y.to_string(),
                    "click",
                    "--repeat",
                    repeat,
                    button,
                ],
            )
            .await?;
            Ok(json!({"x":x,"y":y,"button":button,"clicked":true,"click_count":repeat}))
        }
        "type_text" => {
            let text = string_argument(arguments, "text")?;
            run_output("xdotool", &["type", "--clearmodifiers", "--", text]).await?;
            Ok(json!({"typed":true,"characters":text.chars().count()}))
        }
        "keypress" => {
            let keys = arguments
                .get("keys")
                .and_then(Value::as_array)
                .context("keys are required")?
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>();
            run_output("xdotool", &["key", "--clearmodifiers", &keys.join("+")]).await?;
            Ok(json!({"pressed":true,"keys":keys}))
        }
        "scroll" => {
            let delta = arguments
                .get("scroll_y")
                .and_then(Value::as_f64)
                .context("scroll_y is required")?;
            let button = if delta > 0.0 { "5" } else { "4" };
            let repeat = (delta.abs() / 120.0).ceil().max(1.0) as u32;
            run_output(
                "xdotool",
                &["click", "--repeat", &repeat.to_string(), button],
            )
            .await?;
            Ok(json!({"scrolled":true,"delta":delta}))
        }
        other => bail!("unsupported desktop action {other}"),
    }
}

#[cfg(target_os = "linux")]
fn coordinates(arguments: &Value) -> Result<(i64, i64)> {
    let x = arguments
        .get("x")
        .and_then(Value::as_i64)
        .context("x is required")?;
    let y = arguments
        .get("y")
        .and_then(Value::as_i64)
        .context("y is required")?;
    if !(-100_000..=100_000).contains(&x) || !(-100_000..=100_000).contains(&y) {
        bail!("desktop coordinates are invalid");
    }
    Ok((x, y))
}

#[cfg(target_os = "linux")]
async fn linux_display() -> Result<Value> {
    let output = run_output("xdotool", &["getdisplaygeometry"]).await?;
    let dimensions = output.split_whitespace().collect::<Vec<_>>();
    Ok(
        json!({"x":0,"y":0,"width":dimensions.first().and_then(|v|v.parse::<u32>().ok()).unwrap_or(0),"height":dimensions.get(1).and_then(|v|v.parse::<u32>().ok()).unwrap_or(0),"primary":true,"device_name":"DISPLAY","scale_factor":1}),
    )
}

#[cfg(target_os = "linux")]
fn string_argument<'a>(arguments: &'a Value, name: &str) -> Result<&'a str> {
    arguments
        .get(name)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .with_context(|| format!("{name} is required"))
}

#[cfg(target_os = "linux")]
fn command_available(program: &str) -> bool {
    let finder = if cfg!(windows) { "where" } else { "which" };
    std::process::Command::new(finder)
        .arg(program)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

async fn run_output(program: &str, arguments: &[&str]) -> Result<String> {
    let child = Command::new(program)
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("failed to start {program}"))?;
    let output = timeout(Duration::from_secs(120), child.wait_with_output())
        .await
        .with_context(|| format!("{program} timed out"))??;
    if !output.status.success() {
        bail!(
            "{} failed: {}",
            program,
            String::from_utf8_lossy(&output.stderr)
                .chars()
                .take(4000)
                .collect::<String>()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

pub fn send_keys_text(text: &str) -> String {
    let mut encoded = String::with_capacity(text.len());
    for character in text.chars() {
        if matches!(
            character,
            '+' | '^' | '%' | '~' | '(' | ')' | '{' | '}' | '[' | ']'
        ) {
            encoded.push('{');
            encoded.push(character);
            encoded.push('}');
        } else if character == '\n' {
            encoded.push_str("{ENTER}");
        } else {
            encoded.push(character);
        }
    }
    encoded
}

pub fn send_keys_chord(keys: &[String]) -> Result<String> {
    if keys.is_empty() || keys.len() > 16 {
        bail!("keypress requires between one and 16 keys");
    }
    let mut modifiers = String::new();
    let mut primary = None;
    for key in keys {
        match key.trim().to_ascii_uppercase().as_str() {
            "CTRL" | "CONTROL" => modifiers.push('^'),
            "ALT" => modifiers.push('%'),
            "SHIFT" => modifiers.push('+'),
            "ENTER" | "RETURN" => primary = Some("{ENTER}".to_owned()),
            "TAB" => primary = Some("{TAB}".to_owned()),
            "ESC" | "ESCAPE" => primary = Some("{ESC}".to_owned()),
            "BACKSPACE" => primary = Some("{BACKSPACE}".to_owned()),
            "DELETE" => primary = Some("{DELETE}".to_owned()),
            "UP" | "DOWN" | "LEFT" | "RIGHT" | "HOME" | "END" | "PGUP" | "PGDN" => {
                primary = Some(format!("{{{}}}", key.trim().to_ascii_uppercase()))
            }
            value
                if value.len() == 1
                    && value
                        .chars()
                        .all(|character| character.is_ascii_alphanumeric()) =>
            {
                primary = Some(value.to_ascii_lowercase())
            }
            value
                if value.starts_with('F')
                    && value[1..]
                        .parse::<u8>()
                        .is_ok_and(|number| (1..=24).contains(&number)) =>
            {
                primary = Some(format!("{{{value}}}"))
            }
            other => bail!("unsupported key {other}"),
        }
    }
    let primary = primary.context("keypress is missing a non-modifier key")?;
    Ok(format!("{modifiers}{primary}"))
}

#[cfg(test)]
mod tests {
    use super::{execute, send_keys_chord, send_keys_text};
    use serde_json::json;

    #[test]
    fn send_keys_encoding_escapes_text_and_validates_chords() {
        assert_eq!(send_keys_text("a+b\n"), "a{+}b{ENTER}");
        assert_eq!(send_keys_chord(&["CTRL".into(), "A".into()]).unwrap(), "^a");
        assert!(send_keys_chord(&["CTRL".into()]).is_err());
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn windows_daemon_reads_display_and_captures_a_screenshot() {
        let root =
            std::env::temp_dir().join(format!("open-cowork-desktop-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let screenshot = root.join("screen.png");
        let display = execute("display", &json!({}), &screenshot).await.unwrap();
        assert!(
            display
                .get("width")
                .and_then(|value| value.as_i64())
                .unwrap_or(0)
                > 0
        );
        let captured = execute("screenshot", &json!({}), &screenshot)
            .await
            .unwrap();
        assert_eq!(
            captured.get("path").and_then(|value| value.as_str()),
            screenshot.to_str()
        );
        assert!(screenshot.metadata().unwrap().len() > 100);
        std::fs::remove_dir_all(root).unwrap();
    }
}
