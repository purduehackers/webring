use std::path::Path;

use axum::http::{
    Uri,
    uri::{Authority, PathAndQuery},
};
use sarlacc::Intern;
use serde::Serialize;
use tera::Tera;

#[derive(Clone, Debug)]
pub struct Homepage {
    html: String,
}

impl Homepage {
    pub async fn new(
        static_dir: &Path,
        base_address: Intern<Uri>,
        members: &[MemberForHomepage],
    ) -> eyre::Result<Self> {
        let index_template_path = static_dir.join("index.html");
        let template_content = tokio::fs::read_to_string(&index_template_path).await?;
        let mut tera = Tera::default();
        tera.add_raw_template("index.html", &template_content)?;
        let mut context = tera::Context::new();
        context.insert("members", members);
        context.insert("base_addr", &SerializableUri::from(&*base_address));
        let html = tera.render("index.html", &context)?;
        Ok(Self { html })
    }

    pub fn to_html(&self) -> &str {
        &self.html
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct MemberForHomepage {
    pub name: String,
    pub website: SerializableUri,
    pub check_successful: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq, Hash)]
pub struct SerializableUri {
    /// Scheme name (without `://` part)
    pub scheme: String,
    /// Authority (i.e. host and port)
    pub authority: String,
    /// Path and query string
    pub path_and_query: String,
}

impl From<&Uri> for SerializableUri {
    fn from(value: &Uri) -> Self {
        let scheme = value.scheme_str().unwrap_or("https").to_string();
        let authority = value.authority().map_or("", Authority::as_str).to_string();
        let path_and_query = value
            .path_and_query()
            .map_or("/", PathAndQuery::as_str)
            .to_string();
        Self {
            scheme,
            authority,
            path_and_query,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use axum::http::Uri;
    use pretty_assertions::assert_eq;
    use sarlacc::Intern;
    use tempfile::TempDir;

    use super::{Homepage, MemberForHomepage, SerializableUri};

    #[tokio::test]
    async fn test_render_homepage() {
        // Create index.html template
        let tmpdir = TempDir::new().unwrap();
        let template = r#"
<table>
{% for member in members -%}
<tr><td>{{ member.name }}</td><a href="{{ base_addr.scheme }}://{{ base_addr.authority }}{{ base_addr.path_and_query }}visit?member={{ member.website.authority }}">{{ member.website.authority }}</a></td></tr>
{% endfor -%}
</table>
"#;
        tokio::fs::write(tmpdir.path().join("index.html"), template.as_bytes())
            .await
            .unwrap();

        // Render homepage
        let members = vec![
            MemberForHomepage {
                name: s("kian"),
                website: u("https://kasad.com/"),
                check_successful: true,
            },
            MemberForHomepage {
                name: s("henry"),
                website: u("hrovnyak.gitlab.io"),
                check_successful: true,
            },
        ];
        let homepage = Homepage::new(
            tmpdir.path(),
            Intern::new(Uri::from_static("https://ring.purduehackers.com")),
            &members,
        )
        .await
        .unwrap();
        let html = homepage.to_html();

        assert_eq!(
            r#"
<table>
<tr><td>kian</td><a href="https://ring.purduehackers.com&#x2F;visit?member=kasad.com">kasad.com</a></td></tr>
<tr><td>henry</td><a href="https://ring.purduehackers.com&#x2F;visit?member=hrovnyak.gitlab.io">hrovnyak.gitlab.io</a></td></tr>
</table>
"#,
            html
        );
    }

    /// Create a [`String`] from a `&str`
    fn s(string: &str) -> String {
        string.to_owned()
    }

    /// Create a [`SerializableUri`] from a string
    fn u(s: &str) -> SerializableUri {
        SerializableUri::from(&Uri::from_str(s).unwrap())
    }
}
