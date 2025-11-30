pub mod stream;

/// Describes how to report events during a Vite Task session.
/// It's an abstraction over different kinds of ui (stream, terminial ui, web, etc).
pub trait Reporter {
    /// Report the execution plan that is about to be executed.
    fn report_execution_plan(self: Box<Self>, tree: &str);
}
