use futures_util::future::LocalBoxFuture;

use crate::{assets::AssetProvider, integration::TestDriver, platform, AppContext};

pub struct App;

impl App {
    pub(in crate::platform) fn new(
        _callbacks: platform::app::AppCallbacks,
        _assets: Box<dyn AssetProvider>,
        _test_driver: Option<&TestDriver>,
    ) -> Self {
        unimplemented!();
    }

    pub(in crate::platform) fn run(
        self,
        _init_fn: impl FnOnce(&mut AppContext, LocalBoxFuture<'static, crate::App>),
    ) {
        unimplemented!();
    }
}
