// Optional solaris-auth login for servers with `[auth] require_plugin_login`.
// The plugin replies with literal private system chat: a join/command prompt naming
// the missing step, then `AUTH_UNLOCKED` once the native gate unlocks this connection.
const AUTH_PROMPT_REGISTER: &str =
    "Зарегистрируйтесь /register <пароль> <пароль> / Register: /register <password> <password>";
const AUTH_PROMPT_LOGIN: &str = "Войдите /login <пароль> / Log in: /login <password>";
const AUTH_UNLOCKED: &str = "Вход выполнен / Successfully logged in";
const AUTH_WRONG_PASSWORD: &str = "Неверный пароль / Wrong password";

fn auth_password_parser(value: &str) -> Result<String, String> {
    if (4..=128).contains(&value.len()) && !value.chars().any(char::is_whitespace) {
        Ok(value.to_owned())
    } else {
        Err("password must be 4-128 bytes without whitespace".into())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AuthPrompt {
    Register,
    Login,
}

#[derive(Debug)]
struct BotAuth {
    password: String,
    /// Latest unanswered server prompt; consumed when the matching command is sent.
    prompt: Option<AuthPrompt>,
    unlocked: bool,
}

impl BotAuth {
    fn new(password: String) -> Self {
        Self {
            password,
            prompt: None,
            unlocked: false,
        }
    }
}

impl Bot {
    /// Record solaris-auth replies; every other system chat is ignored here.
    fn observe_auth_chat(&mut self, frame: &RawFrame) -> Result<()> {
        let Some(auth) = self.auth.as_mut().filter(|auth| !auth.unlocked) else {
            return Ok(());
        };
        let packet = ClientboundSystemChat::decode(&mut frame.body.as_ref())?;
        let tag = mc_nbt::read_network(&mut packet.content_nbt.as_slice())?;
        if literal_component_matches(&tag, AUTH_UNLOCKED) {
            auth.unlocked = true;
            auth.prompt = None;
        } else if literal_component_matches(&tag, AUTH_PROMPT_REGISTER) {
            auth.prompt = Some(AuthPrompt::Register);
        } else if literal_component_matches(&tag, AUTH_PROMPT_LOGIN) {
            auth.prompt = Some(AuthPrompt::Login);
        } else if literal_component_matches(&tag, AUTH_WRONG_PASSWORD) {
            bail!(
                "solaris-auth rejected --auth-password for {}",
                self.username
            );
        }
        Ok(())
    }

    /// Answer the server's register/login prompt, then wait for the unlock reply.
    /// Event-driven; bounded by the caller's preparation deadline.
    async fn authenticate(&mut self, client: &mut Client, vd: i32) -> Result<()> {
        loop {
            let Some(auth) = self.auth.as_mut() else {
                return Ok(());
            };
            if auth.unlocked {
                return Ok(());
            }
            if let Some(prompt) = auth.prompt.take() {
                let command = match prompt {
                    AuthPrompt::Register => format!("register {0} {0}", auth.password),
                    AuthPrompt::Login => format!("login {}", auth.password),
                };
                client
                    .write_packet(&ServerboundChatCommand { command })
                    .await?;
            }
            let frame = client.read_frame().await?;
            self.frame(client, frame, vd, true).await?;
        }
    }
}
