use anyhow::Result;
use futures::{Future, TryFutureExt};
use sqlx::{AnyConnection, Connection};
use std::io;
use std::time::Duration;

use crate::opt::{Command, ConnectOpts, DatabaseCommand, MigrateCommand};

mod database;
// mod migration;
// mod migrator;
mod migrate;
mod opt;
mod prepare;

pub use crate::opt::Opt;

pub async fn run(opt: Opt) -> Result<()> {
    match opt.command {
        Command::Migrate(migrate) => match migrate.command {
            MigrateCommand::Add {
                source,
                description,
                reversible,
            } => migrate::add(source.resolve(&migrate.source), &description, reversible).await?,
            MigrateCommand::Run {
                source,
                dry_run,
                ignore_missing,
                connect_opts,
            } => {
                migrate::run(
                    source.resolve(&migrate.source),
                    &connect_opts,
                    dry_run,
                    *ignore_missing,
                )
                .await?
            }
            MigrateCommand::Revert {
                source,
                dry_run,
                ignore_missing,
                connect_opts,
            } => {
                migrate::revert(
                    source.resolve(&migrate.source),
                    &connect_opts,
                    dry_run,
                    *ignore_missing,
                )
                .await?
            }
            MigrateCommand::Info {
                source,
                connect_opts,
            } => migrate::info(source.resolve(&migrate.source), &connect_opts).await?,
            MigrateCommand::BuildScript { source, force } => {
                migrate::build_script(source.resolve(&migrate.source), force)?
            }
        },

        Command::Database(database) => match database.command {
            DatabaseCommand::Create { connect_opts } => database::create(&connect_opts).await?,
            DatabaseCommand::Drop {
                confirmation,
                connect_opts,
            } => database::drop(&connect_opts, !confirmation.yes).await?,
            DatabaseCommand::Reset {
                confirmation,
                source,
                connect_opts,
            } => database::reset(&source, &connect_opts, !confirmation.yes).await?,
            DatabaseCommand::Setup {
                source,
                connect_opts,
            } => database::setup(&source, &connect_opts).await?,
        },

        Command::Prepare {
            check: false,
            merged,
            args,
            connect_opts,
        } => prepare::run(&connect_opts, merged, args).await?,

        Command::Prepare {
            check: true,
            merged,
            args,
            connect_opts,
        } => prepare::check(&connect_opts, merged, args).await?,
    };

    Ok(())
}

/// Attempt to connect to the database server, retrying up to `ops.connect_timeout`.
async fn connect(opts: &ConnectOpts) -> sqlx::Result<AnyConnection> {
    retry_connect_errors(opts, AnyConnection::connect).await
}

/// Attempt an operation that may return errors like `ConnectionRefused`,
/// retrying up until `ops.connect_timeout`.
///
/// The closure is passed `&ops.database_url` for easy composition.
async fn retry_connect_errors<'a, F, Fut, T>(
    opts: &'a ConnectOpts,
    mut connect: F,
) -> sqlx::Result<T>
where
    F: FnMut(&'a str) -> Fut,
    Fut: Future<Output = sqlx::Result<T>> + 'a,
{
    backoff::future::retry(
        backoff::ExponentialBackoffBuilder::new()
            .with_max_elapsed_time(Some(Duration::from_secs(opts.connect_timeout)))
            .build(),
        || {
            connect(&opts.database_url).map_err(|e| -> backoff::Error<sqlx::Error> {
                match e {
                    sqlx::Error::Io(ref ioe) => match ioe.kind() {
                        io::ErrorKind::ConnectionRefused
                        | io::ErrorKind::ConnectionReset
                        | io::ErrorKind::ConnectionAborted => {
                            return backoff::Error::transient(e);
                        }
                        _ => (),
                    },
                    _ => (),
                }

                backoff::Error::permanent(e)
            })
        },
    )
    .await
}
