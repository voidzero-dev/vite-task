#[derive(Default)]
pub struct StreamReporter(());

impl crate::reporter::Reporter for StreamReporter {
    fn report_execution_plan(self: Box<Self>, tree: &str) {}
}
