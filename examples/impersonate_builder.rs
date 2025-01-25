use rquest::{Client, Impersonate, ImpersonateOS};

#[tokio::main]
async fn main() -> Result<(), rquest::Error> {
    // Build a client to impersonate Firefox128
    let impersonate = Impersonate::builder()
        .impersonate(Impersonate::Firefox128)
        .impersonate_os(ImpersonateOS::Windows)
        .skip_http2(false)
        .skip_headers(false)
        .build();

    // Apply the impersonate to the client
    let client = Client::builder()
        .impersonate(impersonate)
        .danger_accept_invalid_certs(true)
        .build()?;

    let text = client
        .get("https://tls.peet.ws/api/all")
        .send()
        .await?
        .text()
        .await?;
    println!("{}", text);

    Ok(())
}
