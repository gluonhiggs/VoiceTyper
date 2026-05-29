//! Groq Whisper transcription: POST in-memory WAV bytes, get back text.

/// POST WAV bytes to Groq's Whisper endpoint and return the transcript text.
/// `language` is the ISO-639-1 code (e.g. `"en"`, `"vi"`); supplying it
/// improves Groq's accuracy and latency vs auto-detect.
pub fn transcribe_bytes(
    audio: Vec<u8>,
    api_key: &str,
    model: &str,
    language: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let part = reqwest::blocking::multipart::Part::bytes(audio)
        .file_name("audio.wav")
        .mime_str("audio/wav")?;
    let form = reqwest::blocking::multipart::Form::new()
        .part("file", part)
        .text("model", model.to_string())
        .text("response_format", "text")
        .text("language", language.to_string())
        .text("temperature", "0");
    let resp = reqwest::blocking::Client::new()
        .post("https://api.groq.com/openai/v1/audio/transcriptions")
        .bearer_auth(api_key)
        .multipart(form)
        .send()?;
    let status = resp.status();
    let body = resp.text()?;
    if !status.is_success() {
        return Err(format!("Groq API {status}: {body}").into());
    }
    Ok(body.trim().to_string())
}
