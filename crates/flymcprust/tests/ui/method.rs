use flymcprust::tool;

struct Calculator;

impl Calculator {
    #[tool]
    fn add(&self, a: i64, b: i64) -> i64 {
        a + b
    }
}

fn main() {}
