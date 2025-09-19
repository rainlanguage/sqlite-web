pub fn sanitize_identifier(name: &str) -> String {
    let s: String = name
        .trim()
        .chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '.' | '_' | '-' => c,
            _ => '_',
        })
        .collect();
    if s.is_empty() {
        "db".to_string()
    } else {
        s
    }
}

pub fn sanitize_db_filename(name: &str) -> String {
    let mut id = sanitize_identifier(name);
    if !id.ends_with(".db") {
        id.push_str(".db");
    }
    id
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_identifier_basic() {
        assert_eq!(sanitize_identifier("test"), "test");
        assert_eq!(sanitize_identifier(" test  "), "test");
        assert_eq!(sanitize_identifier("weird name!*"), "weird_name__");
        assert_eq!(sanitize_identifier(""), "db");
    }

    #[test]
    fn test_sanitize_db_filename() {
        assert_eq!(sanitize_db_filename("mydb"), "mydb.db");
        assert_eq!(sanitize_db_filename("mydb.db"), "mydb.db");
        assert_eq!(sanitize_db_filename("bad/name"), "bad_name.db");
        assert_eq!(sanitize_db_filename(""), "db.db");
    }
}
