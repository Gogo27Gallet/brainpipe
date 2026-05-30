use cfb::CompoundFile;
use std::fs::File;
use std::io::Read;
use std::path::Path;

pub fn extract_msg_pages(path: &Path) -> Result<Vec<crate::office::OfficePage>, String> {
    let file = File::open(path).map_err(|e| format!("Failed to open MSG file: {}", e))?;
    let mut comp = CompoundFile::open(file).map_err(|e| format!("Invalid MSG format: {}", e))?;

    let mut subject = String::new();
    let mut sender = String::new();
    let mut sender_email = String::new();
    let mut body = String::new();

    // Outlook MSG streams are in the root storage.
    // Stream names for properties are named: __substg1.0_XXXXYYYY
    // where XXXX is the property ID (in hex), and YYYY is the property type (in hex).
    // Common property types:
    // 001F: Unicode String (UTF-16-LE)
    // 001E: ASCII/ANSI String
    
    // Properties:
    // Subject: 0037
    // Sender Name: 0042
    // Sender Email: 0076
    // Body Text: 1000
    // Body HTML: 1013
    
    if let Ok(val) = read_msg_string(&mut comp, "/__substg1.0_0037") {
        subject = val;
    }
    if let Ok(val) = read_msg_string(&mut comp, "/__substg1.0_0042") {
        sender = val;
    }
    if let Ok(val) = read_msg_string(&mut comp, "/__substg1.0_0076") {
        sender_email = val;
    }
    if let Ok(val) = read_msg_string(&mut comp, "/__substg1.0_1000") {
        body = val;
    } else if let Ok(html_val) = read_msg_string(&mut comp, "/__substg1.0_1013") {
        body = crate::formats::strip_html_for_extra(&html_val);
    }

    let mut text = String::new();
    if !sender.is_empty() || !sender_email.is_empty() {
        text.push_str(&format!("From: {} <{}>\n", sender.trim(), sender_email.trim()));
    }
    if !subject.is_empty() {
        text.push_str(&format!("Subject: {}\n\n", subject.trim()));
    }
    text.push_str(&body);

    Ok(vec![crate::office::OfficePage {
        label: Some("email".to_string()),
        text,
    }])
}

fn read_msg_string<F: std::io::Read + std::io::Seek>(
    comp: &mut CompoundFile<F>,
    prefix: &str,
) -> Result<String, String> {
    // Try unicode first: prefix + "001F"
    let unicode_path = format!("{}001F", prefix);
    if comp.is_stream(&unicode_path) {
        let mut stream = comp.open_stream(&unicode_path).map_err(|e| e.to_string())?;
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).map_err(|e| e.to_string())?;
        // UTF-16-LE
        let u16_chars: Vec<u16> = buf
            .chunks_exact(2)
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .collect();
        // Remove trailing null byte if any
        let mut clean_u16 = u16_chars;
        if clean_u16.last() == Some(&0) {
            clean_u16.pop();
        }
        return String::from_utf16(&clean_u16).map_err(|e| e.to_string());
    }

    // Try ANSI: prefix + "001E"
    let ansi_path = format!("{}001E", prefix);
    if comp.is_stream(&ansi_path) {
        let mut stream = comp.open_stream(&ansi_path).map_err(|e| e.to_string())?;
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).map_err(|e| e.to_string())?;
        // Remove trailing null byte if any
        if buf.last() == Some(&0) {
            buf.pop();
        }
        return String::from_utf8(buf).map_err(|e| e.to_string());
    }

    Err(format!("Stream not found: {}", prefix))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_invalid_msg_fails_gracefully() {
        let temp_dir = std::env::temp_dir();
        let path = temp_dir.join("corrupt_test.msg");
        {
            let mut file = File::create(&path).unwrap();
            file.write_all(b"corrupt header - not CFB").unwrap();
        }
        let res = extract_msg_pages(&path);
        assert!(res.is_err());
        assert!(res.err().unwrap().contains("Invalid MSG format"));
        let _ = std::fs::remove_file(path);
    }
}
