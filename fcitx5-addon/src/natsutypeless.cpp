#include "natsutypeless.h"

#include <algorithm>
#include <iomanip>
#include <random>
#include <sstream>
#include <string_view>

#include <dbus_public.h>
#include <fcitx-utils/capabilityflags.h>
#include <fcitx-utils/dbus/matchrule.h>
#include <fcitx-utils/dbus/message.h>
#include <fcitx-utils/log.h>
#include <fcitx-utils/utf8.h>
#include <fcitx/addonfactory.h>
#include <fcitx/addonmanager.h>
#include <fcitx/event.h>
#include <fcitx/inputcontextmanager.h>
#include <fcitx/inputpanel.h>
#include <fcitx/surroundingtext.h>
#include <fcitx/text.h>
#include <fcitx/userinterface.h>

namespace {

constexpr const char *kBusName = "io.github.ddy314.NatsuTypeless";
constexpr const char *kObjectPath = "/io/github/ddy314/NatsuTypeless";
constexpr const char *kInterface = "io.github.ddy314.NatsuTypeless";
constexpr uint64_t kCallTimeoutUsec = 5'000'000;

std::string makeSessionId() {
    std::random_device device;
    std::mt19937_64 generator(device());
    std::ostringstream stream;
    stream << std::hex << std::setfill('0');
    for (int i = 0; i < 2; ++i) {
        stream << std::setw(16) << generator();
    }
    return stream.str();
}

std::string jsonEscape(std::string_view value) {
    std::ostringstream out;
    for (const unsigned char c : value) {
        switch (c) {
        case '"':
            out << "\\\"";
            break;
        case '\\':
            out << "\\\\";
            break;
        case '\b':
            out << "\\b";
            break;
        case '\f':
            out << "\\f";
            break;
        case '\n':
            out << "\\n";
            break;
        case '\r':
            out << "\\r";
            break;
        case '\t':
            out << "\\t";
            break;
        default:
            if (c < 0x20) {
                out << "\\u" << std::hex << std::setw(4) << std::setfill('0')
                    << static_cast<int>(c);
            } else {
                out << static_cast<char>(c);
            }
        }
    }
    return out.str();
}

std::string jsonString(std::string_view value) {
    return "\"" + jsonEscape(value) + "\"";
}

std::string jsonStringArray(const std::vector<std::string> &values) {
    std::ostringstream out;
    out << '[';
    for (size_t i = 0; i < values.size(); ++i) {
        if (i) {
            out << ',';
        }
        out << jsonString(values[i]);
    }
    out << ']';
    return out.str();
}

std::pair<std::string, std::string>
boundedContext(fcitx::InputContext *inputContext) {
    const auto &surrounding = inputContext->surroundingText();
    if (!surrounding.isValid() ||
        !fcitx::utf8::validate(surrounding.text())) {
        return {};
    }
    const auto &text = surrounding.text();
    if (text.empty()) {
        return {};
    }
    const size_t length = fcitx::utf8::length(text);
    const size_t cursor = std::min<size_t>(surrounding.cursor(), length);
    const size_t beforeStart = cursor > 256 ? cursor - 256 : 0;
    const size_t afterEnd = std::min(length, cursor + 64);
    auto beforeBegin = fcitx::utf8::nextNChar(text.begin(), beforeStart);
    auto cursorIt = fcitx::utf8::nextNChar(text.begin(), cursor);
    auto afterEndIt = fcitx::utf8::nextNChar(text.begin(), afterEnd);
    return {
        std::string(beforeBegin, cursorIt),
        std::string(cursorIt, afterEndIt),
    };
}

} // namespace

namespace natsu_typeless {

NatsuTypeless::NatsuTypeless(fcitx::Instance *instance)
    : instance_(instance), stateFactory_([](fcitx::InputContext &) {
          return new SessionState;
      }) {
    reloadConfig();
    instance_->inputContextManager().registerProperty("natsuTypelessState",
                                                      &stateFactory_);
    dbusAddon_ = instance_->addonManager().addon("dbus", true);
    if (dbusAddon_) {
        bus_ = dbusAddon_->call<fcitx::IDBusModule::bus>();
    }
    if (!bus_) {
        FCITX_ERROR() << "Natsu Typeless could not access the fcitx DBus module";
        return;
    }

    signalSlots_.emplace_back(bus_->addMatch(
        fcitx::dbus::MatchRule(kBusName, kObjectPath, kInterface,
                               "StateChanged"),
        [this](fcitx::dbus::Message &message) {
            handleStateSignal(message);
            return true;
        }));
    signalSlots_.emplace_back(bus_->addMatch(
        fcitx::dbus::MatchRule(kBusName, kObjectPath, kInterface,
                               "ResultReady"),
        [this](fcitx::dbus::Message &message) {
            handleResultSignal(message);
            return true;
        }));

    eventHandlers_.emplace_back(instance_->watchEvent(
        fcitx::EventType::InputContextKeyEvent,
        fcitx::EventWatcherPhase::PreInputMethod,
        [this](fcitx::Event &event) {
            handleKey(static_cast<fcitx::KeyEvent &>(event));
        }));
    auto reset = [this](fcitx::Event &event) {
        auto &contextEvent = static_cast<fcitx::InputContextEvent &>(event);
        auto *state =
            contextEvent.inputContext()->propertyFor(&stateFactory_);
        if (state->phase != SessionPhase::Idle) {
            cancel(contextEvent.inputContext(), true);
        }
    };
    eventHandlers_.emplace_back(instance_->watchEvent(
        fcitx::EventType::InputContextFocusOut,
        fcitx::EventWatcherPhase::Default, reset));
    eventHandlers_.emplace_back(instance_->watchEvent(
        fcitx::EventType::InputContextReset,
        fcitx::EventWatcherPhase::Default, reset));
    configureDaemon();
}

NatsuTypeless::~NatsuTypeless() = default;

void NatsuTypeless::reloadConfig() {
    fcitx::readAsIni(config_, "conf/natsu-typeless.conf");
    configureDaemon();
}

void NatsuTypeless::setConfig(const fcitx::RawConfig &config) {
    config_.load(config, true);
    fcitx::safeSaveAsIni(config_, "conf/natsu-typeless.conf");
    configureDaemon();
}

void NatsuTypeless::configureDaemon() {
    if (!bus_) {
        return;
    }
    const int timeout = std::clamp(*config_.cloudTimeoutMs, 500, 15000);
    const int idle = std::clamp(*config_.modelIdleMinutes, 1, 120);
    const int maximum = std::clamp(*config_.maxRecordingSeconds, 5, 300);
    std::ostringstream json;
    json << "{\"cloud_base_url\":" << jsonString(*config_.cloudApiBase) << ','
         << "\"cloud_model\":" << jsonString(*config_.cloudModel) << ','
         << "\"cloud_api_key_required\":"
         << (*config_.cloudApiKeyRequired ? "true" : "false") << ','
         << "\"cloud_timeout_ms\":" << timeout << ','
         << "\"model_idle_minutes\":" << idle << ','
         << "\"max_recording_seconds\":" << maximum << '}';
    auto message = bus_->createMethodCall(kBusName, kObjectPath, kInterface,
                                          "Configure");
    message << json.str();
    // Configure is idempotent. The call slot may be discarded after sending;
    // DBus activation and daemon defaults cover startup races.
    configureCall_ = message.callAsync(
        kCallTimeoutUsec, [](fcitx::dbus::Message &) { return true; });
}

bool NatsuTypeless::canStart(fcitx::InputContext *inputContext) const {
    const auto capabilities = inputContext->capabilityFlags();
    if (capabilities.test(fcitx::CapabilityFlag::PasswordOrSensitive) ||
        capabilities.test(fcitx::CapabilityFlag::Disable)) {
        return false;
    }
    return !instance_->isComposing(inputContext);
}

void NatsuTypeless::handleKey(fcitx::KeyEvent &event) {
    auto *inputContext = event.inputContext();
    auto *state = inputContext->propertyFor(&stateFactory_);
    const bool trigger =
        event.key().checkKeyList(config_.triggerKey.value());

    if (state->phase == SessionPhase::Idle) {
        if (!event.isRelease() && trigger && canStart(inputContext)) {
            begin(inputContext, event);
        }
        return;
    }

    if (event.isRelease() && trigger &&
        (state->phase == SessionPhase::Starting ||
         state->phase == SessionPhase::Recording)) {
        event.filterAndAccept();
        end(inputContext, event);
        return;
    }
    if (!event.isRelease() && event.key().check(FcitxKey_Escape)) {
        event.filterAndAccept();
        cancel(inputContext, true);
        return;
    }
    if (state->phase == SessionPhase::Starting ||
        state->phase == SessionPhase::Recording) {
        event.filterAndAccept();
    }
}

void NatsuTypeless::begin(fcitx::InputContext *inputContext,
                          fcitx::KeyEvent &event) {
    if (!bus_) {
        showError(inputContext, "Natsu Typeless daemon is unavailable");
        return;
    }
    event.filterAndAccept();
    auto *state = inputContext->propertyFor(&stateFactory_);
    state->phase = SessionPhase::Starting;
    state->id = makeSessionId();
    sessions_[state->id] = inputContext->watch();
    updateStatus(inputContext, "Starting microphone…");

    std::string contextBefore;
    std::string contextAfter;
    if (*config_.surroundingContext) {
        std::tie(contextBefore, contextAfter) = boundedContext(inputContext);
    }
    std::ostringstream options;
    const auto configuredLanguage = *config_.language;
    const auto language =
        configuredLanguage == "zh" || configuredLanguage == "en"
            ? configuredLanguage
            : std::string("auto");
    options << "{\"language\":" << jsonString(language)
            << ",\"vocabulary\":"
            << jsonStringArray(config_.vocabulary.value())
            << ",\"context_before\":" << jsonString(contextBefore)
            << ",\"context_after\":" << jsonString(contextAfter)
            << ",\"cloud_enabled\":"
            << (*config_.cloudEnabled ? "true" : "false") << '}';

    auto message =
        bus_->createMethodCall(kBusName, kObjectPath, kInterface, "Begin");
    message << state->id << options.str();
    const auto sessionId = state->id;
    auto reference = inputContext->watch();
    state->pendingCall = message.callAsync(
        kCallTimeoutUsec,
        [this, reference, sessionId](fcitx::dbus::Message &reply) mutable {
            auto *inputContext = reference.get();
            if (!inputContext) {
                return true;
            }
            auto *state = inputContext->propertyFor(&stateFactory_);
            if (reply.isError() && state->id == sessionId) {
                showError(inputContext, "Could not start voice input");
                clearSession(inputContext);
            }
            return true;
        });
}

void NatsuTypeless::end(fcitx::InputContext *inputContext,
                        fcitx::KeyEvent &event) {
    event.filterAndAccept();
    auto *state = inputContext->propertyFor(&stateFactory_);
    if (state->id.empty() || !bus_) {
        clearSession(inputContext);
        return;
    }
    state->phase = SessionPhase::Transcribing;
    updateStatus(inputContext, "Transcribing…");
    auto message =
        bus_->createMethodCall(kBusName, kObjectPath, kInterface, "End");
    message << state->id;
    const auto sessionId = state->id;
    auto reference = inputContext->watch();
    state->pendingCall = message.callAsync(
        kCallTimeoutUsec,
        [this, reference, sessionId](fcitx::dbus::Message &reply) mutable {
            auto *inputContext = reference.get();
            if (!inputContext) {
                return true;
            }
            auto *state = inputContext->propertyFor(&stateFactory_);
            if (reply.isError() && state->id == sessionId) {
                showError(inputContext, "Could not stop voice input");
                cancel(inputContext, true);
            }
            return true;
        });
}

void NatsuTypeless::cancel(fcitx::InputContext *inputContext,
                           bool notifyDaemon) {
    auto *state = inputContext->propertyFor(&stateFactory_);
    const auto sessionId = state->id;
    if (notifyDaemon && bus_ && !sessionId.empty()) {
        auto message =
            bus_->createMethodCall(kBusName, kObjectPath, kInterface, "Cancel");
        message << sessionId;
        message.send();
    }
    clearSession(inputContext);
}

void NatsuTypeless::clearSession(fcitx::InputContext *inputContext) {
    auto *state = inputContext->propertyFor(&stateFactory_);
    sessions_.erase(state->id);
    state->phase = SessionPhase::Idle;
    state->id.clear();
    // Do not destroy pendingCall from one of its own callbacks. The slot owns
    // that callback and its captures, so resetting it while it is executing
    // invalidates sessionId/reference before the callback returns. A completed
    // slot is harmless and is replaced at the start of the next D-Bus call (or
    // released with the input context).
    updateStatus(inputContext, "");
}

void NatsuTypeless::updateStatus(fcitx::InputContext *inputContext,
                                 const std::string &text) {
    inputContext->inputPanel().setAuxUp(fcitx::Text(text));
    inputContext->updateUserInterface(
        fcitx::UserInterfaceComponent::InputPanel, true);
}

void NatsuTypeless::showError(fcitx::InputContext *inputContext,
                              const std::string &message) {
    instance_->showCustomInputMethodInformation(inputContext, message);
}

void NatsuTypeless::handleStateSignal(fcitx::dbus::Message &message) {
    std::string sessionId;
    std::string phase;
    std::string detail;
    message >> sessionId >> phase >> detail;
    const auto found = sessions_.find(sessionId);
    if (found == sessions_.end()) {
        return;
    }
    auto *inputContext = found->second.get();
    if (!inputContext) {
        sessions_.erase(found);
        return;
    }
    auto *state = inputContext->propertyFor(&stateFactory_);
    if (state->id != sessionId) {
        return;
    }
    if (phase == "recording") {
        state->phase = SessionPhase::Recording;
        updateStatus(inputContext, "Listening…");
    } else if (phase == "transcribing") {
        state->phase = SessionPhase::Transcribing;
        updateStatus(inputContext, "Transcribing…");
    } else if (phase == "polishing") {
        state->phase = SessionPhase::Polishing;
        updateStatus(inputContext, "Polishing…");
    } else if (phase == "result_ready") {
        state->phase = SessionPhase::ResultReady;
        takeResult(sessionId);
    } else if (phase == "idle") {
        // Older daemons emitted `idle` while handling TakeResult. D-Bus may
        // deliver that signal before the method reply, which used to destroy
        // pendingCall and drop the text before it could be committed.
        if (state->phase != SessionPhase::TakingResult) {
            clearSession(inputContext);
        }
    } else if (phase == "error") {
        showError(inputContext,
                  detail == "audio_failed" ? "No speech audio was captured"
                                           : "Voice input failed");
        cancel(inputContext, true);
    }
}

void NatsuTypeless::handleResultSignal(fcitx::dbus::Message &message) {
    std::string sessionId;
    message >> sessionId;
    takeResult(sessionId);
}

void NatsuTypeless::takeResult(const std::string &sessionId) {
    const auto found = sessions_.find(sessionId);
    if (found == sessions_.end() || !bus_) {
        return;
    }
    auto *inputContext = found->second.get();
    if (!inputContext || !inputContext->hasFocus()) {
        if (inputContext) {
            cancel(inputContext, true);
        } else {
            sessions_.erase(found);
        }
        return;
    }
    auto *state = inputContext->propertyFor(&stateFactory_);
    if (state->phase == SessionPhase::TakingResult) {
        return;
    }
    state->phase = SessionPhase::TakingResult;
    updateStatus(inputContext, "Finishing…");
    auto message =
        bus_->createMethodCall(kBusName, kObjectPath, kInterface, "TakeResult");
    message << sessionId;
    auto reference = inputContext->watch();
    state->pendingCall = message.callAsync(
        kCallTimeoutUsec,
        [this, reference, sessionId](fcitx::dbus::Message &reply) mutable {
            auto *inputContext = reference.get();
            if (!inputContext) {
                sessions_.erase(sessionId);
                return true;
            }
            auto *state = inputContext->propertyFor(&stateFactory_);
            if (state->id != sessionId) {
                return true;
            }
            if (reply.isError() || !inputContext->hasFocus()) {
                showError(inputContext, "Voice input result expired");
                clearSession(inputContext);
                return true;
            }
            std::string text;
            bool usedFallback = false;
            std::string timings;
            reply >> text >> usedFallback >> timings;
            if (!text.empty()) {
                inputContext->commitString(text);
            }
            if (usedFallback) {
                instance_->showCustomInputMethodInformation(
                    inputContext, "Cloud unavailable; inserted local transcript");
            }
            clearSession(inputContext);
            return true;
        });
}

fcitx::AddonInstance *
NatsuTypelessFactory::create(fcitx::AddonManager *manager) {
    return new NatsuTypeless(manager->instance());
}

} // namespace natsu_typeless

FCITX_ADDON_FACTORY_V2(natsutypeless,
                       natsu_typeless::NatsuTypelessFactory)
