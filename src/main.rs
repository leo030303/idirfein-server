use rocket::{
    fs::{FileServer, Options},
    response::Redirect,
};
use ws::Message;

#[macro_use]
extern crate rocket;

#[get("/", rank = 2)]
fn redirect_to_blog() -> Redirect {
    Redirect::permanent(uri!("/blog"))
}

#[get("/", rank = 1)]
fn loro_channel(ws: ws::WebSocket) -> ws::Channel<'static> {
    use rocket::futures::{SinkExt, StreamExt};

    ws.channel(move |mut stream| {
        Box::pin(async move {
            while let Some(message_result) = stream.next().await {
                println!("Loro message: {message_result:?}");
                if let Ok(message) = message_result {
                    let _ = stream
                        .send(Message::text(format!(
                            "Your loro message was: {}",
                            message.into_text().unwrap()
                        )))
                        .await;
                } else {
                    println!("Error with loro message: {message_result:?}");
                }
            }

            Ok(())
        })
    })
}

#[get("/", rank = 1)]
fn sync_channel(ws: ws::WebSocket) -> ws::Channel<'static> {
    use rocket::futures::{SinkExt, StreamExt};

    ws.channel(move |mut stream| {
        Box::pin(async move {
            while let Some(message_result) = stream.next().await {
                println!("Sync message: {message_result:?}");
                if let Ok(message) = message_result {
                    let _ = stream
                        .send(Message::text(format!(
                            "Your sync message was: {}",
                            message.into_text().unwrap()
                        )))
                        .await;
                } else {
                    println!("Error with sync message: {message_result:?}");
                }
            }

            Ok(())
        })
    })
}

#[launch]
fn rocket() -> _ {
    rocket::build()
        .mount("/", routes![redirect_to_blog])
        .mount("/loro", routes![loro_channel])
        .mount("/sync", routes![sync_channel])
        .mount(
            "/blog",
            FileServer::new("/home/leoring/blog_content/www", Options::default()),
        )
}
