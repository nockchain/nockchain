use nockvm::noun::{NounAllocator, D};
use nockvm_macros::tas;
use termimad::MadSkin;
use tracing::error;

use crate::nockapp::driver::{make_driver, IODriverFn};
pub fn markdown() -> IODriverFn {
    make_driver(|handle| async move {
        let skin = MadSkin::default_dark();

        loop {
            match handle.next_effect().await {
                Ok(effect) => {
                    let space = effect.noun_space();
                    let Ok(effect_cell) = unsafe { effect.root() }.in_space(&space).as_cell()
                    else {
                        continue;
                    };
                    if unsafe { effect_cell.head().noun().raw_equals(&D(tas!(b"markdown"))) } {
                        let markdown_text = effect_cell.tail().noun();

                        let text = if let Ok(atom) = markdown_text.in_space(&space).as_atom() {
                            String::from_utf8_lossy(&atom.to_bytes_until_nul()?).to_string()
                        } else {
                            error!("Failed to convert markdown text to string");
                            continue;
                        };
                        tracing::debug!("Markdown text: {}", text);

                        println!("{}", skin.term_text(&text));
                    }
                }
                Err(e) => {
                    error!("Error in markdown driver: {:?}", e);
                    continue;
                }
            }
        }
    })
}
