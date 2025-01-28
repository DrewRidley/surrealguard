//! Management of database events and triggers

use std::collections::HashMap;
use crate::analyzer::model::Type;

/// Contains information about registered events
#[derive(Debug, Default, Clone)]
pub struct EventsContext {
    /// Map of event names to their definitions
    events: HashMap<String, EventDefinition>,
}

#[derive(Debug, Clone)]
pub struct EventDefinition {
    pub event_type: EventType,
    pub trigger_table: String,
    pub condition: Option<Type>,
    pub action: Type,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EventType {
    pub change: ChangeType,
    pub trigger: ChangeTrigger,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ChangeType {
    Create,
    Update,
    Delete
}

#[derive(Debug, Clone, PartialEq)]
pub enum ChangeTrigger {
    Before,
    After
}

impl EventsContext {
    /// Registers a new event definition
    pub fn add_event(&mut self, name: String, definition: EventDefinition) {
        self.events.insert(name, definition);
    }

    /// Gets events for a specific table and event type
    pub fn get_table_events(&self, table: &str, typ: EventType) -> Vec<&EventDefinition> {
        self.events.values()
            .filter(|def| def.trigger_table == table && def.event_type == typ)
            .collect()
    }
}
