use std::fmt::Write;

use axum::http::uri::PathAndQuery;

use crate::webring::MemberForHomepage;

#[derive(Clone, Debug)]
pub struct Homepage {
    html: String,
}

impl Homepage {
    pub async fn new(members: &[MemberForHomepage]) -> eyre::Result<Self> {
        let file = tokio::fs::read_to_string("static/index.html").await?;

        let html = file.replace("{{ . }}", &Self::render_table(members));

        Ok(Homepage { html })
    }

    pub fn to_html(&self) -> &str {
        &self.html
    }

    fn render_table(members: &[MemberForHomepage]) -> String {
        let mut rendered = String::new();

        for member in members {
            let mut rendered_url = String::new();
            rendered_url.push_str(member.website.authority().unwrap().as_str());
            if member
                .website
                .path_and_query()
                .map(PathAndQuery::as_str)
                .is_some_and(|path_query| !path_query.is_empty() && path_query != "/")
            {
                rendered_url.push_str(member.website.path());
            }

            write!(
                &mut rendered,
                r#"<tr{class}><td>{name}</td><td><a href="{href}">{url}</a></td></tr>"#,
                name = member.name,
                href = html_escape::encode_quoted_attribute(&member.website.to_string()),
                url = rendered_url,
                class = if member.check_successful {
                    ""
                } else {
                    r#" class="check-unsuccessful""#
                }
            )
            .unwrap();
        }

        rendered
    }
}
