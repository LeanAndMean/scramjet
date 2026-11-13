//! Sign-in over ACP (`docs/RPC.md` section 6): `authMethods` are `/login`'s own sign-ins and
//! the packages' login programs; `authenticate` runs one, each page it opens sent to the client
//! as a URL-mode elicitation; `logout` removes what they saved. Tokens stay in `auth.json`.
use super::{INTERNAL_ERROR, INVALID_PARAMS, Job, Out, REQUEST_CANCELLED, Settings, lock};
use scramjet::agent::login::{self, LoginProgram, Said};
use scramjet::agent::{Providers, codex, copilot};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
};

const META: &str = "io.github.leanandmean.scramjet";

#[derive(Clone)]
enum Method {
    ChatGpt,
    Copilot,
    Program(LoginProgram),
}

impl Method {
    fn id(&self) -> &str {
        match self {
            Self::ChatGpt => "chatgpt",
            Self::Copilot => "copilot",
            Self::Program(program) => &program.provider,
        }
    }
    /// The provider it signs in to, as `auth.json` names it.
    fn provider(&self) -> &str {
        match self {
            Self::ChatGpt => codex::PROVIDER,
            Self::Copilot => copilot::PROVIDER,
            Self::Program(program) => &program.provider,
        }
    }
    fn name(&self) -> &str {
        match self {
            Self::Program(program) => &program.name,
            _ => login::shown_name(self.provider()),
        }
    }
    fn advertised(&self) -> Value {
        let description = match self {
            Self::ChatGpt => "Sign in with your ChatGPT account in the browser".to_owned(),
            Self::Copilot => "Sign in to GitHub with a device code (an Enterprise server through \
                              _meta's enterprise)"
                .to_owned(),
            Self::Program(program) => format!("Sign in through {}'s package", program.name),
        };
        json!({"id": self.id(), "name": self.name(), "description": description})
    }
}

/// The connection's sign-ins, and those running.
#[derive(Default)]
pub(super) struct SignIn {
    methods: Vec<Method>,
    /// The client offered `elicitation.url`.
    urls: bool,
    /// What stops each `authenticate` running, by its request's id.
    running: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
    elicitations: Arc<AtomicU64>,
}

impl SignIn {
    /// Reads the sign-ins the providers offer now; `urls` whether the client opens pages.
    pub(super) fn read(urls: bool) -> Self {
        let programs = match login::Logins::user() {
            Some(logins) => {
                let providers = Providers::read(Settings::load().models_files(), Some(logins));
                providers.login_programs().to_vec()
            }
            None => Vec::new(),
        };
        let mut methods = vec![Method::ChatGpt, Method::Copilot];
        methods.extend(programs.into_iter().map(Method::Program));
        Self {
            methods,
            urls,
            ..Self::default()
        }
    }

    pub(super) fn advertised(&self) -> Value {
        self.methods.iter().map(Method::advertised).collect()
    }

    /// Runs the sign-in `params` names on a thread of its own and answers when it ends; every
    /// session in `sessions` then reads its providers again.
    pub(super) fn authenticate(
        &self,
        out: &Out,
        id: Value,
        params: &Value,
        sessions: Vec<mpsc::Sender<Job>>,
    ) {
        let wanted = params["methodId"].as_str().unwrap_or_default();
        let Some(method) = self.methods.iter().find(|m| m.id() == wanted).cloned() else {
            let message = format!("no sign-in method {wanted:?}");
            return out.error(&id, INVALID_PARAMS, &message);
        };
        if !self.urls {
            let message = format!(
                "signing in to {} opens a page, which this client cannot (elicitation.url): sign \
                 in from a terminal with /login",
                method.name()
            );
            return out.error(&id, INVALID_PARAMS, &message);
        }
        let Some(logins) = login::Logins::user() else {
            let message = "neither XDG_CONFIG_HOME nor HOME is set";
            return out.error(&id, INTERNAL_ERROR, message);
        };
        let enterprise = match (&method, params["_meta"][META]["enterprise"].as_str()) {
            (Method::Copilot, Some(given)) => match copilot::enterprise_domain(given) {
                Ok(enterprise) => enterprise,
                Err(error) => return out.error(&id, INVALID_PARAMS, &error),
            },
            _ => None,
        };
        let cancel = Arc::<AtomicBool>::default();
        let key = id.to_string();
        lock(&self.running).insert(key.clone(), cancel.clone());
        let (out, running) = (out.clone(), self.running.clone());
        let counter = self.elicitations.clone();
        std::thread::spawn(move || {
            let mut pages = Pages {
                out: &out,
                request: &id,
                cancel: &cancel,
                counter: &counter,
                opened: Vec::new(),
            };
            let signed_in = sign_in(&method, &logins, enterprise, &mut pages);
            pages.close();
            lock(&running).remove(&key);
            match signed_in {
                Ok(warning) => {
                    for jobs in sessions {
                        let _ = jobs.send(Job::Credentials(method.provider().to_owned()));
                    }
                    let result = match warning {
                        Some(warning) => {
                            eprintln!("scramjet: Warning: {warning}");
                            json!({"_meta": {META: {"warning": warning}}})
                        }
                        None => json!({}),
                    };
                    out.respond(&id, result);
                }
                Err(error) if error == login::CANCELLED => {
                    out.error(&id, REQUEST_CANCELLED, "the sign-in was cancelled")
                }
                Err(error) => {
                    let message = format!("signing in to {} failed: {error}", method.name());
                    out.error(&id, INTERNAL_ERROR, &message);
                }
            }
        });
    }

    /// Stops the `authenticate` of request `id`, if it runs.
    pub(super) fn cancel(&self, id: &Value) {
        if let Some(cancel) = lock(&self.running).get(&id.to_string()) {
            cancel.store(true, Ordering::Relaxed);
        }
    }

    /// Removes what the sign-ins saved, or only that of `_meta`'s `methodId`; every session in
    /// `sessions` then reads its providers again.
    pub(super) fn logout(
        &self,
        out: &Out,
        id: &Value,
        params: &Value,
        sessions: Vec<mpsc::Sender<Job>>,
    ) {
        let chosen: Vec<&Method> = match params["_meta"][META]["methodId"].as_str() {
            Some(wanted) => match self.methods.iter().find(|m| m.id() == wanted) {
                Some(method) => vec![method],
                None => {
                    let message = format!("no sign-in method {wanted:?}");
                    return out.error(id, INVALID_PARAMS, &message);
                }
            },
            None => self.methods.iter().collect(),
        };
        let Some(logins) = login::Logins::user() else {
            let message = "neither XDG_CONFIG_HOME nor HOME is set";
            return out.error(id, INTERNAL_ERROR, message);
        };
        for method in chosen {
            if let Err(error) = login::logout(&logins, method.provider()) {
                return out.error(id, INTERNAL_ERROR, &error);
            }
            for jobs in &sessions {
                let _ = jobs.send(Job::Credentials(method.provider().to_owned()));
            }
        }
        out.respond(id, json!({}));
    }
}

/// Runs `method`'s sign-in, its pages sent to the client; with a warning if it gave one.
fn sign_in(
    method: &Method,
    logins: &login::Logins,
    enterprise: Option<String>,
    pages: &mut Pages,
) -> Result<Option<String>, String> {
    match method {
        Method::ChatGpt => {
            let browser = codex::BrowserLogin::start()?;
            if !browser.listening() {
                return Err(
                    "port 1455 is in use, so the browser cannot come back: sign in from \
                            a terminal with /login, which takes its last address pasted"
                        .to_owned(),
                );
            }
            pages.open(&browser.url, "Sign in to ChatGPT in the browser");
            // Nothing is pasted here: the browser comes back by itself.
            let (_paste, pasted) = mpsc::channel();
            browser.finish(&pasted, &AtomicBool::new(false), logins, pages.cancel)
        }
        Method::Copilot => {
            let device = copilot::DeviceLogin::start(enterprise)?;
            let message = format!(
                "Enter the code {} on GitHub's page to sign in to GitHub Copilot",
                device.code
            );
            pages.open(&device.url, &message);
            device.finish(logins, pages.cancel).map(|()| None)
        }
        Method::Program(program) => {
            let mut said = None;
            let cancel = pages.cancel.clone();
            program.login(logins, &cancel, |heard| match heard {
                Said::Open(url) => {
                    let message = (said.take())
                        .unwrap_or_else(|| format!("Sign in to {} in the browser", program.name));
                    pages.open(&url, &message);
                }
                Said::Say(text) => said = Some(text),
            })
        }
    }
}

/// The pages a sign-in has the client open, each a URL-mode elicitation scoped to its
/// `authenticate` request; declining one stops the sign-in.
struct Pages<'a> {
    out: &'a Out,
    request: &'a Value,
    cancel: &'a Arc<AtomicBool>,
    counter: &'a AtomicU64,
    /// Each elicitation's request id and `elicitationId`.
    opened: Vec<(Value, String)>,
}

impl Pages<'_> {
    fn open(&mut self, url: &str, message: &str) {
        let elicitation = format!(
            "sign-in-{}",
            self.counter.fetch_add(1, Ordering::Relaxed) + 1
        );
        let params = json!({
            "requestId": self.request,
            "mode": "url",
            "elicitationId": elicitation,
            "url": url,
            "message": message,
        });
        let (id, answered) = self.out.request("elicitation/create", params);
        let cancel = self.cancel.clone();
        // Ends without an answer once `close` forgets the request.
        std::thread::spawn(move || {
            if let Ok(answer) = answered.recv()
                && answer["result"]["action"] != "accept"
            {
                cancel.store(true, Ordering::Relaxed);
            }
        });
        self.opened.push((id, elicitation));
    }

    /// Tells the client the pages are done with.
    fn close(self) {
        for (id, elicitation) in self.opened {
            self.out.forget(&id);
            let params = json!({"elicitationId": elicitation});
            let notification =
                json!({"jsonrpc": "2.0", "method": "elicitation/complete", "params": params});
            self.out.send(&notification);
        }
    }
}
