use super::{IntoProp, Prop, Props};
use crate::request::Request;

/// The page being rendered, as given to a [`ProvidesProps`].
#[derive(Debug, Clone, Copy)]
pub struct RenderContext<'a> {
    /// The page component.
    pub component: &'a str,
    /// The request.
    pub request: &'a Request,
}

/// The prop being resolved, as given to a [`ProvidesProp`].
#[derive(Debug, Clone, Copy)]
pub struct PropertyContext<'a> {
    /// The prop's path, in dot notation (`"auth.user"`).
    pub key: &'a str,
    /// The prop's unresolved siblings, itself included.
    pub props: &'a Props,
    /// The request.
    pub request: &'a Request,
    pub(crate) component: &'a str,
}

/// A source of several props, which may depend on the page being rendered.
///
/// Add its props to a render with [`Response::with_props_from`](crate::Response::with_props_from),
/// share them with [`Inertia::share_from`](crate::Inertia::share_from) or
/// [`Config::share_from`](crate::Config::share_from), or nest them under a
/// key with [`props_from`](crate::props_from). Closures taking a
/// [`RenderContext`] are providers too.
///
/// ```
/// use inertia::{Props, ProvidesProps, RenderContext, props};
///
/// struct Breadcrumbs;
///
/// impl ProvidesProps for Breadcrumbs {
///     fn props(&self, context: &RenderContext<'_>) -> Props {
///         props! { "breadcrumbs" => context.component.split('/').collect::<Vec<_>>() }
///     }
/// }
/// ```
pub trait ProvidesProps {
    /// The props to add to the page.
    fn props(&self, context: &RenderContext<'_>) -> Props;
}

impl<F> ProvidesProps for F
where
    F: Fn(&RenderContext<'_>) -> Props,
{
    fn props(&self, context: &RenderContext<'_>) -> Props {
        self(context)
    }
}

/// A prop computed from its context: its key, its siblings and the request.
///
/// Turn one into a prop with [`contextual`](crate::contextual). Closures
/// taking a [`PropertyContext`] are providers too.
pub trait ProvidesProp {
    /// The prop's value.
    fn prop(&self, context: &PropertyContext<'_>) -> Prop;
}

impl<F, P> ProvidesProp for F
where
    F: Fn(&PropertyContext<'_>) -> P,
    P: IntoProp,
{
    fn prop(&self, context: &PropertyContext<'_>) -> Prop {
        self(context).into_prop()
    }
}
