//! Handles rendering the webring homepage

use std::path::Path;

use axum::http::{
    Uri,
    uri::{Authority, PathAndQuery},
};
use sarlacc::Intern;
use serde::Serialize;
use tera::Tera;

/// Represents the rendered webring homepage
#[derive(Clone, Debug)]
pub struct Homepage {
    /// The rendered HTML content of the homepage
    html: String,
}

impl Homepage {
    /// Creates a new `Homepage` instance by reading the `index.html` template from the specified
    /// directory and rendering it with the provided members and base address.
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

    /// Returns the rendered HTML content of the homepage.
    pub fn to_html(&self) -> &str {
        &self.html
    }
}

/// Represents the information about a member to be displayed on the homepage.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct MemberForHomepage {
    /// Member's name
    pub name: String,
    /// Member's website URI
    pub website: SerializableUri,
    /// Whether the member's website check was successful
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
    /// Full URL, in the format `{scheme}://{authority}{path_and_query}`
    pub href: String,
}

impl From<&Uri> for SerializableUri {
    fn from(value: &Uri) -> Self {
        let scheme = value.scheme_str().unwrap_or("https").to_string();
        let authority = value.authority().map_or("", Authority::as_str).to_string();
        let path_and_query = value
            .path_and_query()
            .map_or("/", PathAndQuery::as_str)
            .to_string();
        let href = format!("{scheme}://{authority}{path_and_query}");
        Self {
            scheme,
            authority,
            path_and_query,
            href,
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
<tr><td>{{ member.name }}</td><a href="{{ base_addr.href }}visit?member={{ member.website.authority }}">{{ member.website.authority }}</a></td></tr>
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
<tr><td>kian</td><a href="https:&#x2F;&#x2F;ring.purduehackers.com&#x2F;visit?member=kasad.com">kasad.com</a></td></tr>
<tr><td>henry</td><a href="https:&#x2F;&#x2F;ring.purduehackers.com&#x2F;visit?member=hrovnyak.gitlab.io">hrovnyak.gitlab.io</a></td></tr>
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
