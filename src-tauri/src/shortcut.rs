use std::path::Path;

fn escape_single_quoted_ps(input: &str) -> String {
    input.replace('\'', "''")
}

pub fn build_shortcut_ps_script(target_path: &Path, link_path: &Path, icon_path: &Path) -> String {
    let target = escape_single_quoted_ps(&target_path.to_string_lossy());
    let link = escape_single_quoted_ps(&link_path.to_string_lossy());
    let icon = escape_single_quoted_ps(&icon_path.to_string_lossy());

    format!(
        "$TargetPath = '{target}'\n\
$LinkPath = '{link}'\n\
$IconPath = '{icon}'\n\
$WshShell = New-Object -ComObject WScript.Shell\n\
$Shortcut = $WshShell.CreateShortcut($LinkPath)\n\
$Shortcut.TargetPath = $TargetPath\n\
$Shortcut.WorkingDirectory = Split-Path -Path $TargetPath -Parent\n\
$Shortcut.IconLocation = $IconPath\n\
$Shortcut.Save()\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn shortcut_ps_script_is_deterministic() {
        let target = PathBuf::from(r"C:\Program Files\Jarvis\app.exe");
        let link = PathBuf::from(r"C:\Users\tester\Desktop\jarvis-desktop.lnk");
        let icon = PathBuf::from(r"C:\Program Files\Jarvis\app.exe");

        let script = build_shortcut_ps_script(&target, &link, &icon);
        let expected = "$TargetPath = 'C:\\Program Files\\Jarvis\\app.exe'\n\
$LinkPath = 'C:\\Users\\tester\\Desktop\\jarvis-desktop.lnk'\n\
$IconPath = 'C:\\Program Files\\Jarvis\\app.exe'\n\
$WshShell = New-Object -ComObject WScript.Shell\n\
$Shortcut = $WshShell.CreateShortcut($LinkPath)\n\
$Shortcut.TargetPath = $TargetPath\n\
$Shortcut.WorkingDirectory = Split-Path -Path $TargetPath -Parent\n\
$Shortcut.IconLocation = $IconPath\n\
$Shortcut.Save()\n";
        assert_eq!(script, expected);
    }

    #[test]
    fn shortcut_ps_script_escapes_single_quotes() {
        let target = PathBuf::from(r"C:\Apps\jarvis'\app.exe");
        let link = PathBuf::from(r"C:\Users\tester\Desktop\jarvis-desktop.lnk");
        let icon = PathBuf::from(r"C:\Apps\jarvis'\app.exe");

        let script = build_shortcut_ps_script(&target, &link, &icon);
        assert!(script.contains("jarvis''\\app.exe"));
    }
}
