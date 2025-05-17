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
