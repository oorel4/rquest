mod support;
use support::server;
use tokio::io::AsyncWriteExt;

#[tokio::test]
async fn zstd_response() {
    zstd_case(10_000, 4096).await;
}

#[tokio::test]
async fn zstd_single_byte_chunks() {
    zstd_case(10, 1).await;
}

#[tokio::test]
async fn test_zstd_empty_body() {
    let server = server::http(move |req| async move {
        assert_eq!(req.method(), "HEAD");

        http::Response::builder()
            .header("content-encoding", "zstd")
            .body(Default::default())
            .unwrap()
    });

    let client = rquest::Client::new();
    let res = client
        .head(format!("http://{}/zstd", server.addr()))
        .send()
        .await
        .unwrap();

    let body = res.text().await.unwrap();

    assert_eq!(body, "");
}

#[tokio::test]
async fn test_accept_header_is_not_changed_if_set() {
    let server = server::http(move |req| async move {
        assert_eq!(req.headers()["accept"], "application/json");
        assert!(req.headers()["accept-encoding"]
            .to_str()
            .unwrap()
            .contains("zstd"));
        http::Response::default()
    });

    let client = rquest::Client::new();

    let res = client
        .get(format!("http://{}/accept", server.addr()))
        .header(
            rquest::header::ACCEPT,
            rquest::header::HeaderValue::from_static("application/json"),
        )
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), rquest::StatusCode::OK);
}

#[tokio::test]
async fn test_accept_encoding_header_is_not_changed_if_set() {
    let server = server::http(move |req| async move {
        assert_eq!(req.headers()["accept"], "*/*");
        assert_eq!(req.headers()["accept-encoding"], "identity");
        http::Response::default()
    });

    let client = rquest::Client::new();

    let res = client
        .get(format!("http://{}/accept-encoding", server.addr()))
        .header(rquest::header::ACCEPT, "*/*")
        .header(
            rquest::header::ACCEPT_ENCODING,
            rquest::header::HeaderValue::from_static("identity"),
        )
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), rquest::StatusCode::OK);
}

async fn zstd_case(response_size: usize, chunk_size: usize) {
    use futures_util::stream::StreamExt;

    let content: String = (0..response_size).fold(String::new(), |mut acc, i| {
        acc.push_str(&format!("test {i}"));
        acc
    });

    let zstded_content = zstd::encode_all(content.as_bytes(), 3).unwrap();

    let mut response = format!(
        "\
         HTTP/1.1 200 OK\r\n\
         Server: test-accept\r\n\
         Content-Encoding: zstd\r\n\
         Content-Length: {}\r\n\
         \r\n",
        &zstded_content.len()
    )
    .into_bytes();
    response.extend(&zstded_content);

    let server = server::http(move |req| {
        assert!(req.headers()["accept-encoding"]
            .to_str()
            .unwrap()
            .contains("zstd"));

        let zstded = zstded_content.clone();
        async move {
            let len = zstded.len();
            let stream =
                futures_util::stream::unfold((zstded, 0), move |(zstded, pos)| async move {
                    let chunk = zstded.chunks(chunk_size).nth(pos)?.to_vec();

                    Some((chunk, (zstded, pos + 1)))
                });

            let body = rquest::Body::wrap_stream(stream.map(Ok::<_, std::convert::Infallible>));

            http::Response::builder()
                .header("content-encoding", "zstd")
                .header("content-length", len)
                .body(body)
                .unwrap()
        }
    });

    let client = rquest::Client::new();

    let res = client
        .get(format!("http://{}/zstd", server.addr()))
        .send()
        .await
        .expect("response");

    let body = res.text().await.expect("text");
    assert_eq!(body, content);
}

const COMPRESSED_RESPONSE_HEADERS: &[u8] = b"HTTP/1.1 200 OK\x0d\x0a\
            Content-Type: text/plain\x0d\x0a\
            Connection: keep-alive\x0d\x0a\
            Content-Encoding: zstd\x0d\x0a";

const RESPONSE_CONTENT: &str = "some message here";

fn zstd_compress(input: &[u8]) -> Vec<u8> {
    zstd::encode_all(input, 3).unwrap()
}

#[tokio::test]
async fn test_non_chunked_non_fragmented_response() {
    let server = server::low_level_with_response(|_raw_request, client_socket| {
        Box::new(async move {
            let zstded_content = zstd_compress(RESPONSE_CONTENT.as_bytes());
            let content_length_header =
                format!("Content-Length: {}\r\n\r\n", zstded_content.len()).into_bytes();
            let response = [
                COMPRESSED_RESPONSE_HEADERS,
                &content_length_header,
                &zstded_content,
            ]
            .concat();

            client_socket
                .write_all(response.as_slice())
                .await
                .expect("response write_all failed");
            client_socket.flush().await.expect("response flush failed");
        })
    });

    let res = rquest::Client::new()
        .get(format!("http://{}/", server.addr()))
        .send()
        .await
        .expect("response");

    assert_eq!(res.text().await.expect("text"), RESPONSE_CONTENT);
}

#[tokio::test]
async fn test_chunked_fragmented_response_1() {
    const DELAY_BETWEEN_RESPONSE_PARTS: tokio::time::Duration =
        tokio::time::Duration::from_millis(1000);
    const DELAY_MARGIN: tokio::time::Duration = tokio::time::Duration::from_millis(50);

    let server = server::low_level_with_response(|_raw_request, client_socket| {
        Box::new(async move {
            let zstded_content = zstd_compress(RESPONSE_CONTENT.as_bytes());
            let response_first_part = [
                COMPRESSED_RESPONSE_HEADERS,
                format!(
                    "Transfer-Encoding: chunked\r\n\r\n{:x}\r\n",
                    zstded_content.len()
                )
                .as_bytes(),
                &zstded_content,
            ]
            .concat();
            let response_second_part = b"\r\n0\r\n\r\n";

            client_socket
                .write_all(response_first_part.as_slice())
                .await
                .expect("response_first_part write_all failed");
            client_socket
                .flush()
                .await
                .expect("response_first_part flush failed");

            tokio::time::sleep(DELAY_BETWEEN_RESPONSE_PARTS).await;

            client_socket
                .write_all(response_second_part)
                .await
                .expect("response_second_part write_all failed");
            client_socket
                .flush()
                .await
                .expect("response_second_part flush failed");
        })
    });

    let start = tokio::time::Instant::now();
    let res = rquest::Client::new()
        .get(format!("http://{}/", server.addr()))
        .send()
        .await
        .expect("response");

    assert_eq!(res.text().await.expect("text"), RESPONSE_CONTENT);
    assert!(start.elapsed() >= DELAY_BETWEEN_RESPONSE_PARTS - DELAY_MARGIN);
}

#[tokio::test]
async fn test_chunked_fragmented_response_2() {
    const DELAY_BETWEEN_RESPONSE_PARTS: tokio::time::Duration =
        tokio::time::Duration::from_millis(1000);
    const DELAY_MARGIN: tokio::time::Duration = tokio::time::Duration::from_millis(50);

    let server = server::low_level_with_response(|_raw_request, client_socket| {
        Box::new(async move {
            let zstded_content = zstd_compress(RESPONSE_CONTENT.as_bytes());
            let response_first_part = [
                COMPRESSED_RESPONSE_HEADERS,
                format!(
                    "Transfer-Encoding: chunked\r\n\r\n{:x}\r\n",
                    zstded_content.len()
                )
                .as_bytes(),
                &zstded_content,
                b"\r\n",
            ]
            .concat();
            let response_second_part = b"0\r\n\r\n";

            client_socket
                .write_all(response_first_part.as_slice())
                .await
                .expect("response_first_part write_all failed");
            client_socket
                .flush()
                .await
                .expect("response_first_part flush failed");

            tokio::time::sleep(DELAY_BETWEEN_RESPONSE_PARTS).await;

            client_socket
                .write_all(response_second_part)
                .await
                .expect("response_second_part write_all failed");
            client_socket
                .flush()
                .await
                .expect("response_second_part flush failed");
        })
    });

    let start = tokio::time::Instant::now();
    let res = rquest::Client::new()
        .get(format!("http://{}/", server.addr()))
        .send()
        .await
        .expect("response");

    assert_eq!(res.text().await.expect("text"), RESPONSE_CONTENT);
    assert!(start.elapsed() >= DELAY_BETWEEN_RESPONSE_PARTS - DELAY_MARGIN);
}

#[tokio::test]
async fn test_chunked_fragmented_response_with_extra_bytes() {
    const DELAY_BETWEEN_RESPONSE_PARTS: tokio::time::Duration =
        tokio::time::Duration::from_millis(1000);
    const DELAY_MARGIN: tokio::time::Duration = tokio::time::Duration::from_millis(50);

    let server = server::low_level_with_response(|_raw_request, client_socket| {
        Box::new(async move {
            let zstded_content = zstd_compress(RESPONSE_CONTENT.as_bytes());
            let response_first_part = [
                COMPRESSED_RESPONSE_HEADERS,
                format!(
                    "Transfer-Encoding: chunked\r\n\r\n{:x}\r\n",
                    zstded_content.len()
                )
                .as_bytes(),
                &zstded_content,
            ]
            .concat();
            let response_second_part = b"\r\n2ab\r\n0\r\n\r\n";

            client_socket
                .write_all(response_first_part.as_slice())
                .await
                .expect("response_first_part write_all failed");
            client_socket
                .flush()
                .await
                .expect("response_first_part flush failed");

            tokio::time::sleep(DELAY_BETWEEN_RESPONSE_PARTS).await;

            client_socket
                .write_all(response_second_part)
                .await
                .expect("response_second_part write_all failed");
            client_socket
                .flush()
                .await
                .expect("response_second_part flush failed");
        })
    });

    let start = tokio::time::Instant::now();
    let res = rquest::Client::new()
        .get(format!("http://{}/", server.addr()))
        .send()
        .await
        .expect("response");

    let err = res.text().await.expect_err("there must be an error");
    assert!(err.is_decode());
    assert!(start.elapsed() >= DELAY_BETWEEN_RESPONSE_PARTS - DELAY_MARGIN);
}
