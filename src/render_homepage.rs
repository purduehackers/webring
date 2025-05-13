use std::fmt::Write;

use crate::webring::MemberForHomepage;

pub struct Homepage {
    html: String,
}

impl Homepage {
    pub async fn new(members: Vec<MemberForHomepage>) -> eyre::Result<Self> {
        let file = tokio::fs::read_to_string("static/index.html").await?;

        let mut rendered = String::new();

        for member in members {
            write!(
                &mut rendered,
                "<tr{pass_class}><th>{name}</th><th><a href={link}>{link}</a></th></tr>",
                name = member.name,
                link = member.website,
                pass_class = if member.check_successful {
                    ""
                } else {
                    " class=\"check-unsuccessful\""
                }
            )
            .unwrap();
        }

        let html = file.replace("{{ . }}", &rendered);

        Ok(Homepage { html })
    }
}
