FROM rust:latest

WORKDIR /lab/

COPY . .

RUN cargo install --path .

CMD [ "moai_bot" ]