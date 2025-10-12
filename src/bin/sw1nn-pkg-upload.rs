use std::env;
use std::path::Path;
use std::process;

#[tokio::main]
async fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() != 2 {
        eprintln!("Usage: {} <package.pkg.tar.zst>", args[0]);
        process::exit(1);
    }

    let pkg_file = &args[1];
    let path = Path::new(pkg_file);

    if !path.exists() {
        eprintln!("Error: File '{}' does not exist", pkg_file);
        process::exit(1);
    }

    if !pkg_file.ends_with(".pkg.tar.zst") {
        eprintln!("Error: File must be a .pkg.tar.zst package");
        process::exit(1);
    }

    let url = "https://repo.sw1nn.net/api/packages";

    println!("Uploading {} to {}", pkg_file, url);

    let client = reqwest::Client::new();
    let file = match tokio::fs::read(pkg_file).await {
        Ok(f) => f,
        Err(e) => {
            eprintln!("Error reading file: {}", e);
            process::exit(1);
        }
    };

    let file_name = path.file_name().unwrap().to_string_lossy().to_string();
    let part = reqwest::multipart::Part::bytes(file)
        .file_name(file_name);

    let form = reqwest::multipart::Form::new()
        .part("file", part);

    match client.post(url).multipart(form).send().await {
        Ok(response) => {
            if response.status().is_success() {
                println!("Successfully uploaded package");
                match response.text().await {
                    Ok(body) => println!("{}", body),
                    Err(e) => eprintln!("Error reading response: {}", e),
                }
            } else {
                eprintln!("Upload failed with status: {}", response.status());
                match response.text().await {
                    Ok(body) => eprintln!("Error: {}", body),
                    Err(e) => eprintln!("Error reading response: {}", e),
                }
                process::exit(1);
            }
        }
        Err(e) => {
            eprintln!("Error uploading package: {}", e);
            process::exit(1);
        }
    }
}
