use crate::mod_manager::types::{Mod, ModState};
use iced::widget::text;
use iced::Element;

#[derive(Debug, Clone)]
pub enum Message {
    Scan,
    SearchChanged(String),
    TabSelected(usize),
    MoveToArchive(String),
    MoveToActive(String),
    RemoveMod(String),
    Exit,
}

pub fn mod_list<'a>(mods: &'a [Mod]) -> Element<'a, Message> {
    let mut list = iced::widget::Column::new();
    
    for mod_data in mods {
        list = list.push(
            iced::widget::row![
                text(&mod_data.name).size(16),
                text(format!("{:?}", mod_data.state)).size(12)
            ]
        );
    }
    
    iced::widget::scrollable(list).into()
}