//! Launch origins paired with the shipped production gateway. Origin separation is retained.
use crate::error::ApiError;
use reqwest::Url;
use uuid::Uuid;

fn invalid() -> ApiError {
    ApiError::Unprocessable("invalid or unconfigured instance gateway origin".into())
}

fn domain(value: &str) -> bool {
    value.len() <= 219
        && value.contains('.')
        && value != "localhost"
        && !value.ends_with(".localhost")
        && value.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
        })
}

pub(crate) fn origin(instance: Uuid) -> Result<Url, ApiError> {
    let template = std::env::var("COWORK_INSTANCE_URL_TEMPLATE").map_err(|_| invalid())?;
    if template.matches("{instance}").count() != 1 {
        return Err(invalid());
    }
    let url = Url::parse(&template.replace("{instance}", &instance.simple().to_string()))
        .map_err(|_| invalid())?;
    let loopback = url
        .host_str()
        .is_some_and(|host| host == "localhost" || host.ends_with(".localhost"));
    if !(url.scheme() == "https" || url.scheme() == "http" && loopback)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid());
    }
    if let Ok(configured) = std::env::var("COWORK_INSTANCE_GATEWAY_DOMAIN") {
        if !domain(&configured)
            || template != format!("https://{{instance}}.{configured}")
            || url.host_str() != Some(format!("{}.{configured}", instance.simple()).as_str())
            || url.scheme() != "https"
            || url.port().is_some()
        {
            return Err(invalid());
        }
    }
    Ok(url)
}

pub(crate) fn validate_configuration() -> anyhow::Result<()> {
    if std::env::var_os("COWORK_INSTANCE_GATEWAY_DOMAIN").is_some() {
        origin(Uuid::nil()).map_err(|_| {
            anyhow::anyhow!(
                "Production instance gateway domain and launch URL template do not match"
            )
        })?;
    }
    Ok(())
}
