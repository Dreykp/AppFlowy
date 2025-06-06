#![allow(unused_doc_comments)]

use collab_integrate::collab_builder::AppFlowyCollabBuilder;
use collab_plugins::CollabKVDB;
use flowy_ai::ai_manager::AIManager;
use flowy_database2::DatabaseManager;
use flowy_document::manager::DocumentManager;
use flowy_error::{FlowyError, FlowyResult};
use flowy_folder::manager::FolderManager;
use flowy_search::folder::indexer::FolderIndexManagerImpl;
use flowy_search::services::manager::SearchManager;
use flowy_server::af_cloud::define::LoggedUser;
use std::path::PathBuf;
use std::sync::{Arc, Weak};
use std::time::Duration;
use sysinfo::System;
use tokio::sync::RwLock;
use tracing::{debug, error, event, info, instrument};
use uuid::Uuid;

use flowy_sqlite::kv::KVStorePreferences;
use flowy_storage::manager::StorageManager;
use flowy_user::services::authenticate_user::AuthenticateUser;
use flowy_user::services::entities::UserConfig;
use flowy_user::user_manager::UserManager;

use lib_dispatch::prelude::*;
use lib_dispatch::runtime::AFPluginRuntime;
use lib_infra::priority_task::{TaskDispatcher, TaskRunner};
use lib_infra::util::OperatingSystem;
use lib_log::stream_log::StreamLogSender;
use module::make_plugins;

use crate::config::AppFlowyCoreConfig;
use crate::deps_resolve::file_storage_deps::FileStorageResolver;
use crate::deps_resolve::*;
use crate::log_filter::init_log;
use crate::server_layer::ServerProvider;
use deps_resolve::reminder_deps::CollabInteractImpl;
use flowy_sqlite::DBConnection;
use lib_infra::async_trait::async_trait;
use user_state_callback::UserStatusCallbackImpl;

pub mod config;
mod deps_resolve;
mod log_filter;
pub mod module;
pub(crate) mod server_layer;
pub(crate) mod user_state_callback;

/// This name will be used as to identify the current [AppFlowyCore] instance.
/// Don't change this.
pub const DEFAULT_NAME: &str = "appflowy";

#[derive(Clone)]
pub struct AppFlowyCore {
  #[allow(dead_code)]
  pub config: AppFlowyCoreConfig,
  pub user_manager: Arc<UserManager>,
  pub document_manager: Arc<DocumentManager>,
  pub folder_manager: Arc<FolderManager>,
  pub database_manager: Arc<DatabaseManager>,
  pub event_dispatcher: Arc<AFPluginDispatcher>,
  pub server_provider: Arc<ServerProvider>,
  pub task_dispatcher: Arc<RwLock<TaskDispatcher>>,
  pub store_preference: Arc<KVStorePreferences>,
  pub search_manager: Arc<SearchManager>,
  pub ai_manager: Arc<AIManager>,
  pub storage_manager: Arc<StorageManager>,
}

impl AppFlowyCore {
  pub async fn new(
    config: AppFlowyCoreConfig,
    runtime: Arc<AFPluginRuntime>,
    stream_log_sender: Option<Arc<dyn StreamLogSender>>,
  ) -> Self {
    let platform = OperatingSystem::from(&config.platform);

    #[allow(clippy::if_same_then_else)]
    if cfg!(debug_assertions) {
      /// The profiling can be used to tracing the performance of the application.
      /// Check out the [Link](https://docs.appflowy.io/docs/documentation/software-contributions/architecture/backend/profiling#enable-profiling)
      ///  for more information.
      #[cfg(feature = "profiling")]
      console_subscriber::init();

      // Init the logger before anything else
      #[cfg(not(feature = "profiling"))]
      init_log(&config, &platform, stream_log_sender);
    } else {
      init_log(&config, &platform, stream_log_sender);
    }

    if sysinfo::IS_SUPPORTED_SYSTEM {
      info!(
        "💡{:?}, platform: {:?}",
        System::long_os_version(),
        platform
      );
    }

    Self::init(config, runtime).await
  }

  pub fn close_db(&self) {
    self.user_manager.close_db();
  }

  #[instrument(skip(config, runtime))]
  async fn init(config: AppFlowyCoreConfig, runtime: Arc<AFPluginRuntime>) -> Self {
    config.ensure_path();

    // Init the key value database
    let store_preference = Arc::new(KVStorePreferences::new(&config.storage_path).unwrap());
    info!("🔥{:?}", &config);

    let task_scheduler = TaskDispatcher::new(Duration::from_secs(10));
    let task_dispatcher = Arc::new(RwLock::new(task_scheduler));
    runtime.spawn(TaskRunner::run(task_dispatcher.clone()));

    let user_config = UserConfig::new(
      &config.name,
      &config.storage_path,
      &config.application_path,
      &config.device_id,
      config.app_version.clone(),
    );

    let authenticate_user = Arc::new(AuthenticateUser::new(
      user_config.clone(),
      store_preference.clone(),
    ));

    debug!("🔥runtime:{}", runtime);

    let server_provider = Arc::new(ServerProvider::new(
      config.clone(),
      Arc::downgrade(&store_preference),
      ServerUserImpl(Arc::downgrade(&authenticate_user)),
    ));

    event!(tracing::Level::DEBUG, "Init managers",);
    let (
      user_manager,
      folder_manager,
      server_provider,
      database_manager,
      document_manager,
      collab_builder,
      search_manager,
      ai_manager,
      storage_manager,
    ) = async {
      let storage_manager = FileStorageResolver::resolve(
        Arc::downgrade(&authenticate_user),
        server_provider.clone(),
        &user_config.storage_path,
      );
      /// The shared collab builder is used to build the [Collab] instance. The plugins will be loaded
      /// on demand based on the [CollabPluginConfig].
      let collab_builder = Arc::new(AppFlowyCollabBuilder::new(
        server_provider.clone(),
        WorkspaceCollabIntegrateImpl(Arc::downgrade(&authenticate_user)),
      ));

      collab_builder
        .set_snapshot_persistence(Arc::new(SnapshotDBImpl(Arc::downgrade(&authenticate_user))));

      let folder_indexer = Arc::new(FolderIndexManagerImpl::new(Arc::downgrade(
        &authenticate_user,
      )));

      let folder_manager = FolderDepsResolver::resolve(
        Arc::downgrade(&authenticate_user),
        collab_builder.clone(),
        server_provider.clone(),
        folder_indexer.clone(),
        store_preference.clone(),
      )
      .await;

      let folder_query_service = FolderServiceImpl::new(
        Arc::downgrade(&folder_manager),
        Arc::downgrade(&authenticate_user),
      );

      let ai_manager = ChatDepsResolver::resolve(
        Arc::downgrade(&authenticate_user),
        server_provider.clone(),
        store_preference.clone(),
        Arc::downgrade(&storage_manager.storage_service),
        server_provider.clone(),
        folder_query_service.clone(),
        server_provider.local_ai.clone(),
      );

      let database_manager = DatabaseDepsResolver::resolve(
        Arc::downgrade(&authenticate_user),
        task_dispatcher.clone(),
        collab_builder.clone(),
        server_provider.clone(),
        server_provider.clone(),
        ai_manager.clone(),
      )
      .await;

      let document_manager = DocumentDepsResolver::resolve(
        Arc::downgrade(&authenticate_user),
        &database_manager,
        collab_builder.clone(),
        server_provider.clone(),
        Arc::downgrade(&storage_manager.storage_service),
      );

      let user_manager = UserDepsResolver::resolve(
        authenticate_user.clone(),
        collab_builder.clone(),
        server_provider.clone(),
        store_preference.clone(),
        database_manager.clone(),
        folder_manager.clone(),
      )
      .await;

      let search_manager = SearchDepsResolver::resolve(
        folder_indexer,
        server_provider.clone(),
        folder_manager.clone(),
      )
      .await;

      // Register the folder operation handlers
      register_handlers(
        &folder_manager,
        document_manager.clone(),
        database_manager.clone(),
        ai_manager.clone(),
      );

      (
        user_manager,
        folder_manager,
        server_provider,
        database_manager,
        document_manager,
        collab_builder,
        search_manager,
        ai_manager,
        storage_manager,
      )
    }
    .await;

    let user_status_callback = UserStatusCallbackImpl {
      user_manager: user_manager.clone(),
      collab_builder,
      folder_manager: folder_manager.clone(),
      database_manager: database_manager.clone(),
      document_manager: document_manager.clone(),
      server_provider: server_provider.clone(),
      storage_manager: storage_manager.clone(),
      ai_manager: ai_manager.clone(),
      runtime: runtime.clone(),
    };

    let collab_interact_impl = CollabInteractImpl {
      database_manager: Arc::downgrade(&database_manager),
      document_manager: Arc::downgrade(&document_manager),
    };

    let cloned_user_manager = Arc::downgrade(&user_manager);
    if let Some(user_manager) = cloned_user_manager.upgrade() {
      if let Err(err) = user_manager
        .init_with_callback(user_status_callback, collab_interact_impl)
        .await
      {
        error!("Init user failed: {}", err)
      }
    }
    #[allow(clippy::arc_with_non_send_sync)]
    let event_dispatcher = Arc::new(AFPluginDispatcher::new(
      runtime,
      make_plugins(
        Arc::downgrade(&folder_manager),
        Arc::downgrade(&database_manager),
        Arc::downgrade(&user_manager),
        Arc::downgrade(&document_manager),
        Arc::downgrade(&search_manager),
        Arc::downgrade(&ai_manager),
        Arc::downgrade(&storage_manager),
      ),
    ));

    Self {
      config,
      user_manager,
      document_manager,
      folder_manager,
      database_manager,
      event_dispatcher,
      server_provider,
      task_dispatcher,
      store_preference,
      search_manager,
      ai_manager,
      storage_manager,
    }
  }

  /// Only expose the dispatcher in test
  pub fn dispatcher(&self) -> Arc<AFPluginDispatcher> {
    self.event_dispatcher.clone()
  }
}

struct ServerUserImpl(Weak<AuthenticateUser>);

impl ServerUserImpl {
  fn upgrade_user(&self) -> Result<Arc<AuthenticateUser>, FlowyError> {
    let user = self
      .0
      .upgrade()
      .ok_or(FlowyError::internal().with_context("Unexpected error: UserSession is None"))?;
    Ok(user)
  }
}

#[async_trait]
impl LoggedUser for ServerUserImpl {
  fn workspace_id(&self) -> FlowyResult<Uuid> {
    self.upgrade_user()?.workspace_id()
  }

  fn user_id(&self) -> FlowyResult<i64> {
    self.upgrade_user()?.user_id()
  }

  async fn is_local_mode(&self) -> FlowyResult<bool> {
    self.upgrade_user()?.is_local_mode().await
  }

  fn get_sqlite_db(&self, uid: i64) -> Result<DBConnection, FlowyError> {
    self.upgrade_user()?.get_sqlite_connection(uid)
  }

  fn get_collab_db(&self, uid: i64) -> Result<Weak<CollabKVDB>, FlowyError> {
    self.upgrade_user()?.get_collab_db(uid)
  }

  fn application_root_dir(&self) -> Result<PathBuf, FlowyError> {
    Ok(PathBuf::from(
      self.upgrade_user()?.get_application_root_dir(),
    ))
  }
}
