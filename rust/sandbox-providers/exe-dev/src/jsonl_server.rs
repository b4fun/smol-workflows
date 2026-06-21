use crate::error::{invalid_request, provider_failure};
use crate::provider::{EventWriter, ExeDevProvider, ProviderAction};
use serde_json::Value;
use smol_workflow_sandbox::{JsonlRequestEnvelope, JsonlResponseEnvelope, ProviderError};
use tokio::io::{self, AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};

pub async fn serve_stdio() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    serve(
        BufReader::new(stdin),
        stdout,
        ExeDevProvider::from_environment()?,
    )
    .await
}

pub async fn serve<R, W>(
    reader: R,
    mut writer: W,
    mut provider: ExeDevProvider,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut lines = reader.lines();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }

        let request: Result<JsonlRequestEnvelope<Value>, ProviderError> =
            serde_json::from_str(&line).map_err(|source| {
                invalid_request(format!("failed to parse JSONL request: {source}"))
            });

        let request = match request {
            Ok(request) => request,
            Err(error) => {
                write_response(
                    &mut writer,
                    &JsonlResponseEnvelope {
                        id: "unknown".to_string(),
                        result: None,
                        error: Some(error),
                        event: None,
                    },
                )
                .await?;
                continue;
            }
        };

        let result = {
            let mut events = EventWriter::new(&request.id, &mut writer);
            provider
                .handle_with_events(&request.id, &request.method, request.params, &mut events)
                .await
        };

        match result {
            Ok(ProviderAction::Respond(result)) => {
                write_response(&mut writer, &success_response(request.id, result)).await?;
            }
            Ok(ProviderAction::Shutdown(result)) => {
                write_response(&mut writer, &success_response(request.id, result)).await?;
                break;
            }
            Err(error) => {
                write_response(
                    &mut writer,
                    &JsonlResponseEnvelope {
                        id: request.id,
                        result: None,
                        error: Some(error),
                        event: None,
                    },
                )
                .await?;
            }
        }
    }
    Ok(())
}

fn success_response(id: String, result: Value) -> JsonlResponseEnvelope {
    JsonlResponseEnvelope {
        id,
        result: Some(result),
        error: None,
        event: None,
    }
}

async fn write_response<W>(writer: &mut W, response: &JsonlResponseEnvelope) -> io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    let mut line = serde_json::to_vec(response).map_err(|source| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            provider_failure(source.to_string()),
        )
    })?;
    line.push(b'\n');
    writer.write_all(&line).await?;
    writer.flush().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use tokio::io::BufReader;

    #[tokio::test]
    async fn serve_loop_handles_capabilities_and_shutdown() {
        let input = br#"{"id":"req_1","method":"capabilities","params":{}}
{"id":"req_2","method":"shutdown","params":{}}
"#;
        let mut output = Vec::new();
        serve(
            BufReader::new(&input[..]),
            &mut output,
            ExeDevProvider::new(Config::default()),
        )
        .await
        .unwrap();
        let lines = String::from_utf8(output).unwrap();
        assert!(lines.contains(r#"{"id":"req_1","result":{"exec":true}}"#));
        assert!(lines.contains(r#"{"id":"req_2","result":{}}"#));
    }
}
