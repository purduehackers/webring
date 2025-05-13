use std::fmt::Write;

use crate::webring::MemberForHomepage;

pub struct Homepage {
    html: String,
}

impl Homepage {
    pub async fn new(members: Vec<MemberForHomepage>) -> eyre::Result<Self> {
        let file = tokio::fs::read_to_string("static/index.html").await?;

        let html = file.replace("{{ . }}", &Self::render_table(members));

        Ok(Homepage { html })
    }

    fn render_table(members: Vec<MemberForHomepage>) -> String {
        let mut rendered = String::new();

        for member in members {
            write!(
                &mut rendered,
                "<tr{class}><th>{name}</th><th><a href={link}>{link}</a></th></tr>",
                name = member.name,
                link = member.website.authority().unwrap(),
                class = if member.check_successful {
                    ""
                } else {
                    " class=\"check-unsuccessful\""
                }
            )
            .unwrap();
        }

        rendered
    }
}
