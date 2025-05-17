use std::path::Path;

use tera::Tera;

use crate::webring::MemberForHomepage;

#[derive(Clone, Debug)]
pub struct Homepage {
    html: String,
}

impl Homepage {
    pub async fn new(static_dir: &Path, members: &[MemberForHomepage]) -> eyre::Result<Self> {
        let index_template_path = static_dir.join("index.html");
        let template_content = tokio::fs::read_to_string(&index_template_path).await?;
        let mut tera = Tera::default();
        tera.add_raw_template("index.html", &template_content)?;
        let mut context = tera::Context::new();
        context.insert("members", members);
        let html = tera.render("index.html", &context)?;
        Ok(Self { html })
    }

    pub fn to_html(&self) -> &str {
        &self.html
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;
    use tempfile::TempDir;

    use crate::webring::MemberForHomepage;

    use super::Homepage;

    #[tokio::test]
    async fn test_render_homepage() {
        // Create index.html template
        let tmpdir = TempDir::new().unwrap();
        let template = r#"
<table>
{% for member in members %}
<tr><td>{{ member.name }}</td><a href="{{ member.href }}">{{ member.url }}</a></td></tr>
{% endfor %}
</table>
"#;
        tokio::fs::write(tmpdir.path().join("index.html"), template.as_bytes())
            .await
            .unwrap();

        // Render homepage
        let members = vec![
            MemberForHomepage {
                name: s("kian"),
                href: s("https://kasad.com/"),
                url: s("kasad.com"),
                check_successful: true,
            },
            MemberForHomepage {
                name: s("henry"),
                href: s("https://hrovnyak.gitlab.io/"),
                url: s("hrovnyak.gitlab.io"),
                check_successful: true,
            },
        ];
        let homepage = Homepage::new(tmpdir.path(), &members).await.unwrap();
        let html = homepage.to_html();

        assert_eq!(
            r#"
<table>

<tr><td>kian</td><a href="https:&#x2F;&#x2F;kasad.com&#x2F;">kasad.com</a></td></tr>

<tr><td>henry</td><a href="https:&#x2F;&#x2F;hrovnyak.gitlab.io&#x2F;">hrovnyak.gitlab.io</a></td></tr>

</table>
"#,
            html
        );
    }

    /// Create a [`String`] from a `&str`
    fn s(string: &str) -> String {
        string.to_owned()
    }
}
