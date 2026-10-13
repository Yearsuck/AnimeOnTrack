use super::*;

pub fn get_site_priority_core(db: &Db, known: &[&str]) -> Result<Vec<String>, String> {
    db.get_site_priority(known).map_err(|e| e.to_string())
}

pub fn set_site_priority_core(db: &Db, order: Vec<String>, known: &[&str]) -> Result<(), String> {
    let mut seen = HashSet::new();
    for id in &order {
        if !known.contains(&id.as_str()) {
            return Err(format!("sitio desconocido: {id}"));
        }
        if !seen.insert(id.as_str()) {
            return Err(format!("sitio duplicado: {id}"));
        }
    }
    let mut complete = order.clone();
    for &id in known {
        if seen.insert(id) {
            complete.push(id.to_string());
        }
    }
    db.set_site_priority(&complete).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_site_priority(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    let known: Vec<&str> = adapter::all_sites().iter().map(|s| s.id).collect();
    let db = state.db.lock().unwrap();
    get_site_priority_core(&db, &known)
}

#[tauri::command]
pub fn set_site_priority(state: State<'_, AppState>, order: Vec<String>) -> Result<(), String> {
    let known: Vec<&str> = adapter::all_sites().iter().map(|s| s.id).collect();
    let db = state.db.lock().unwrap();
    set_site_priority_core(&db, order, &known)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_set_site_priority_rejects_unknown_id() {
        let db = Db::open(":memory:").unwrap();
        let known = ["a", "b", "c"];
        let res = set_site_priority_core(&db, vec!["unknown".to_string()], &known);
        assert!(res.is_err());
        assert_eq!(res.unwrap_err(), "sitio desconocido: unknown");
        assert_eq!(db.get_setting("site_priority").unwrap(), None);
    }

    #[test]
    fn test_set_site_priority_rejects_duplicate_id() {
        let db = Db::open(":memory:").unwrap();
        let known = ["a", "b", "c"];
        let res = set_site_priority_core(&db, vec!["a".to_string(), "a".to_string()], &known);
        assert!(res.is_err());
        assert_eq!(res.unwrap_err(), "sitio duplicado: a");
        assert_eq!(db.get_setting("site_priority").unwrap(), None);
    }

    #[test]
    fn test_set_site_priority_appends_missing_known_sites() {
        let db = Db::open(":memory:").unwrap();
        let known = ["a", "b", "c"];
        set_site_priority_core(&db, vec!["c".to_string(), "a".to_string()], &known).unwrap();
        let priority = get_site_priority_core(&db, &known).unwrap();
        assert_eq!(priority, vec!["c", "a", "b"]);
    }
}
