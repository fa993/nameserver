pub mod server;
use std::{fmt::Display, io};

use actix_web::{
    get, http::StatusCode, post, web, App, HttpResponse, HttpServer, Responder, ResponseError,
};
use log::{debug, info};
use sea_orm::{
    entity::prelude::*,
    sea_query::{Expr, Query},
    Database, Schema,
};

const BRANCH_FACTOR: u32 = 6;

#[derive(Debug)]
enum NameserverError {
    ORMError(sea_orm::error::DbErr),
    ParentNotFound,
    IOError(io::Error),
}

impl From<io::Error> for NameserverError {
    fn from(value: io::Error) -> Self {
        Self::IOError(value)
    }
}

impl From<DbErr> for NameserverError {
    fn from(value: DbErr) -> Self {
        NameserverError::ORMError(value)
    }
}

impl Display for NameserverError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl ResponseError for NameserverError {
    fn status_code(&self) -> actix_web::http::StatusCode {
        StatusCode::INTERNAL_SERVER_ERROR
    }

    fn error_response(&self) -> HttpResponse<actix_web::body::BoxBody> {
        HttpResponse::InternalServerError().body(format!("{self}"))
    }
}

#[get("/")]
async fn hello() -> impl Responder {
    HttpResponse::Ok().body("Hello world!")
}

#[post("/register")]
async fn register(
    state: web::Data<DatabaseConnection>,
    service_id: String,
    body: String,
) -> Result<impl Responder, NameserverError> {
    debug!("{body} wants to register itself");
    //find it in table first
    let en = server::Entity::find()
        .filter(
            server::Column::Id.in_subquery(
                Query::select()
                    .expr(Expr::col(server::Column::Id).div(BRANCH_FACTOR))
                    .from(server::Entity)
                    .and_where(server::Column::Url.eq(&body))
                    .to_owned(),
            ),
        )
        .one(state.as_ref())
        .await?;
    if let Some(t) = en {
        debug!("{body} is already registered so sending the same url back");
        Ok(HttpResponse::Ok().json(t.to_body()))
    } else {
        //insert and then fetch url at id / 5
        let res = server::ActiveModel {
            url: sea_orm::ActiveValue::Set(body.clone()),
            service_id: sea_orm::ActiveValue::Set(service_id),
            ..Default::default()
        }
        .insert(state.as_ref())
        .await?;

        debug!("{body} has been inserted succesfully");
        let idd = res.id - 1;
        if idd == 0 {
            //first registration.. no need to connect to anything
            debug!("First registration for {body}");
            Ok(HttpResponse::Ok().body(""))
        } else {
            let id_to_find = idd as u32 / BRANCH_FACTOR;
            let op = server::Entity::find_by_id(id_to_find as i32 + 1)
                .one(state.as_ref())
                .await?
                .ok_or(NameserverError::ParentNotFound)?;
            debug!("{body} parent was found ");
            Ok(HttpResponse::Ok().json(op.to_body()))
        }
    }
}

#[actix_web::main]
async fn main() -> Result<(), NameserverError> {
    pretty_env_logger::init();

    dotenv::dotenv().expect("DotEnv threw error");

    let conn_str = std::env::var("NAMESERVER_CONNECT_STRING")
        .ok()
        .unwrap_or("postgres://root:root@localhost:5432/nameserver".to_string());

    info!("Connecting to {}", conn_str);

    let db: DatabaseConnection = Database::connect(&conn_str)
        .await
        .expect(&format!("Could not connect to DB {}", &conn_str));

    info!("Connected to {}", conn_str);

    let tcs = Schema::new(db.get_database_backend())
        .create_table_from_entity(server::Entity)
        .if_not_exists()
        .to_owned();
    let s = db.get_database_backend().build(&tcs);
    debug!("{s}");
    let _ = db.execute(s).await?;

    HttpServer::new(move || {
        App::new()
            .app_data(web::Data::new(db.clone()))
            .service(register)
            .service(hello)
    })
    .bind((
        "0.0.0.0",
        option_env!("PORT")
            .and_then(|f| f.parse().ok())
            .unwrap_or(8080),
    ))?
    .run()
    .await?;
    Ok(())
}
