fn helper(x: i32) -> i32 { x + 1 }
fn target(x: i32) -> i32 { helper(x) }
#[test]
fn test_target() { target(1); }
